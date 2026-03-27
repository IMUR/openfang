//! Usage tracking store backed by SurrealDB.
//!
//! Tracks LLM token usage, tool calls, and costs per agent.

use chrono::Utc;
use openfang_types::agent::AgentId;
use openfang_types::error::{OpenFangError, OpenFangResult};
use serde::{Deserialize, Serialize};

use crate::db::SurrealDb;

/// Usage store backed by SurrealDB.
#[derive(Clone)]
pub struct UsageStore {
    db: SurrealDb,
}

/// A single usage event record.
#[derive(Debug, Serialize, Deserialize)]
pub struct UsageRecord {
    pub agent_id: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub event_type: String,
    pub created_at: String,
}

/// Aggregated usage summary.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UsageSummary {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub event_count: u64,
}

fn surreal_err(e: surrealdb::Error) -> OpenFangError {
    OpenFangError::Memory(e.to_string())
}

impl UsageStore {
    /// Create a new usage store wrapping the given SurrealDB handle.
    pub fn new(db: SurrealDb) -> Self {
        Self { db }
    }

    /// Record a usage event.
    pub async fn record(
        &self,
        agent_id: AgentId,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
        event_type: &str,
    ) -> OpenFangResult<()> {
        let _: Option<UsageRecord> = self
            .db
            .create("usage")
            .content(UsageRecord {
                agent_id: agent_id.0.to_string(),
                provider: provider.to_string(),
                model: model.to_string(),
                input_tokens,
                output_tokens,
                cost_usd,
                event_type: event_type.to_string(),
                created_at: Utc::now().to_rfc3339(),
            })
            .await
            .map_err(surreal_err)?;
        Ok(())
    }

    /// Get aggregated usage for an agent.
    pub async fn summary(&self, agent_id: AgentId) -> OpenFangResult<UsageSummary> {
        let mut result = self
            .db
            .query(
                "SELECT
                    math::sum(input_tokens) AS total_input_tokens,
                    math::sum(output_tokens) AS total_output_tokens,
                    math::sum(cost_usd) AS total_cost_usd,
                    count() AS event_count
                 FROM usage
                 WHERE agent_id = $aid
                 GROUP ALL",
            )
            .bind(("aid", agent_id.0.to_string()))
            .await
            .map_err(surreal_err)?;

        let summaries: Vec<UsageSummary> = result.take(0).unwrap_or_default();
        Ok(summaries.into_iter().next().unwrap_or_default())
    }

    /// Get recent usage events for an agent.
    pub async fn recent(
        &self,
        agent_id: AgentId,
        limit: usize,
    ) -> OpenFangResult<Vec<UsageRecord>> {
        let mut result = self
            .db
            .query("SELECT * FROM usage WHERE agent_id = $aid ORDER BY created_at DESC LIMIT $lim")
            .bind(("aid", agent_id.0.to_string()))
            .bind(("lim", limit as u64))
            .await
            .map_err(surreal_err)?;

        let records: Vec<UsageRecord> = result.take(0).unwrap_or_default();
        Ok(records)
    }

    /// Delete all usage records for an agent.
    pub async fn clear(&self, agent_id: AgentId) -> OpenFangResult<()> {
        self.db
            .query("DELETE usage WHERE agent_id = $aid")
            .bind(("aid", agent_id.0.to_string()))
            .await
            .map_err(surreal_err)?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // Time-window cost queries (used by MeteringEngine)
    // -----------------------------------------------------------------

    /// Total cost for an agent in the last hour.
    pub async fn query_hourly(&self, agent_id: AgentId) -> OpenFangResult<f64> {
        self.cost_since(Some(agent_id), chrono::Duration::hours(1))
            .await
    }

    /// Total cost for an agent today (UTC).
    pub async fn query_daily(&self, agent_id: AgentId) -> OpenFangResult<f64> {
        self.cost_since(Some(agent_id), chrono::Duration::hours(24))
            .await
    }

    /// Total cost for an agent in the last 30 days.
    pub async fn query_monthly(&self, agent_id: AgentId) -> OpenFangResult<f64> {
        self.cost_since(Some(agent_id), chrono::Duration::days(30))
            .await
    }

    /// Total cost across all agents in the last hour.
    pub async fn query_global_hourly(&self) -> OpenFangResult<f64> {
        self.cost_since(None, chrono::Duration::hours(1)).await
    }

    /// Total cost across all agents today.
    pub async fn query_today_cost(&self) -> OpenFangResult<f64> {
        self.cost_since(None, chrono::Duration::hours(24)).await
    }

    /// Total cost across all agents in the last 30 days.
    pub async fn query_global_monthly(&self) -> OpenFangResult<f64> {
        self.cost_since(None, chrono::Duration::days(30)).await
    }

    /// Get a usage summary, optionally filtered by agent.
    pub async fn query_summary(&self, agent_id: Option<AgentId>) -> OpenFangResult<UsageSummary> {
        let (sql, binds) = match agent_id {
            Some(aid) => (
                "SELECT math::sum(input_tokens) AS total_input_tokens,
                        math::sum(output_tokens) AS total_output_tokens,
                        math::sum(cost_usd) AS total_cost_usd,
                        count() AS event_count
                 FROM usage WHERE agent_id = $aid GROUP ALL",
                Some(("aid", aid.0.to_string())),
            ),
            None => (
                "SELECT math::sum(input_tokens) AS total_input_tokens,
                        math::sum(output_tokens) AS total_output_tokens,
                        math::sum(cost_usd) AS total_cost_usd,
                        count() AS event_count
                 FROM usage GROUP ALL",
                None,
            ),
        };
        let mut q = self.db.query(sql);
        if let Some((k, v)) = binds {
            q = q.bind((k, v));
        }
        let mut result = q.await.map_err(surreal_err)?;
        let summaries: Vec<UsageSummary> = result.take(0).unwrap_or_default();
        Ok(summaries.into_iter().next().unwrap_or_default())
    }

    /// Get usage grouped by model.
    pub async fn query_by_model(&self) -> OpenFangResult<Vec<ModelUsage>> {
        let mut result = self
            .db
            .query(
                "SELECT model,
                        math::sum(input_tokens) AS input_tokens,
                        math::sum(output_tokens) AS output_tokens,
                        math::sum(cost_usd) AS cost_usd,
                        count() AS call_count
                 FROM usage GROUP BY model",
            )
            .await
            .map_err(surreal_err)?;
        let rows: Vec<ModelUsage> = result.take(0).unwrap_or_default();
        Ok(rows)
    }

    /// Delete usage records older than `days` days. Returns count deleted (best-effort).
    pub async fn cleanup_old(&self, days: u32) -> OpenFangResult<usize> {
        let cutoff = (Utc::now() - chrono::Duration::days(days as i64)).to_rfc3339();
        self.db
            .query("DELETE usage WHERE created_at < $cutoff")
            .bind(("cutoff", cutoff))
            .await
            .map_err(surreal_err)?;
        // SurrealDB DELETE doesn't easily return count; return 0 as best-effort
        Ok(0)
    }

    // Internal: sum cost_usd since a given duration, optionally per agent.
    async fn cost_since(
        &self,
        agent_id: Option<AgentId>,
        duration: chrono::Duration,
    ) -> OpenFangResult<f64> {
        let since = (Utc::now() - duration).to_rfc3339();
        let (sql, binds) = match agent_id {
            Some(aid) => (
                "SELECT math::sum(cost_usd) AS total FROM usage WHERE agent_id = $aid AND created_at >= $since GROUP ALL",
                vec![("aid", aid.0.to_string()), ("since", since)],
            ),
            None => (
                "SELECT math::sum(cost_usd) AS total FROM usage WHERE created_at >= $since GROUP ALL",
                vec![("since", since)],
            ),
        };
        let mut q = self.db.query(sql);
        for (k, v) in binds {
            q = q.bind((k, v));
        }
        let mut result = q.await.map_err(surreal_err)?;

        #[derive(Deserialize)]
        struct Row {
            total: Option<f64>,
        }
        let rows: Vec<Row> = result.take(0).unwrap_or_default();
        Ok(rows.into_iter().next().and_then(|r| r.total).unwrap_or(0.0))
    }
}

/// Usage breakdown per model (returned by `query_by_model`).
#[derive(Debug, Serialize, Deserialize)]
pub struct ModelUsage {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub call_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn setup() -> UsageStore {
        let db = db::init_mem().await.unwrap();
        UsageStore::new(db)
    }

    #[tokio::test]
    async fn test_record_and_summary() {
        let store = setup().await;
        let agent_id = AgentId::new();

        store
            .record(agent_id, "anthropic", "claude", 100, 50, 0.01, "chat")
            .await
            .unwrap();
        store
            .record(agent_id, "anthropic", "claude", 200, 100, 0.02, "chat")
            .await
            .unwrap();

        let summary = store.summary(agent_id).await.unwrap();
        assert_eq!(summary.total_input_tokens, 300);
        assert_eq!(summary.total_output_tokens, 150);
        assert!((summary.total_cost_usd - 0.03).abs() < 0.001);
        assert_eq!(summary.event_count, 2);
    }

    #[tokio::test]
    async fn test_recent_events() {
        let store = setup().await;
        let agent_id = AgentId::new();

        for i in 0..5 {
            store
                .record(agent_id, "openai", "gpt-4", i * 10, i * 5, 0.0, "chat")
                .await
                .unwrap();
        }

        let recent = store.recent(agent_id, 3).await.unwrap();
        assert_eq!(recent.len(), 3);
    }

    #[tokio::test]
    async fn test_clear() {
        let store = setup().await;
        let agent_id = AgentId::new();
        store
            .record(agent_id, "anthropic", "claude", 100, 50, 0.01, "chat")
            .await
            .unwrap();
        store.clear(agent_id).await.unwrap();
        let summary = store.summary(agent_id).await.unwrap();
        assert_eq!(summary.event_count, 0);
    }
}
