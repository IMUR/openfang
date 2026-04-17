//! Usage tracking store backed by SurrealDB.
//!
//! Tracks LLM token usage, tool calls, and costs per agent.

use chrono::Utc;
use openfang_types::agent::AgentId;
use openfang_types::error::{OpenFangError, OpenFangResult};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;
use std::collections::HashMap;

use crate::db::SurrealDb;

/// Usage store backed by SurrealDB.
#[derive(Clone)]
pub struct UsageStore {
    db: SurrealDb,
}

/// A single usage event record.
#[derive(Debug, Serialize, Deserialize, SurrealValue)]
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
#[derive(Debug, Default, Serialize, Deserialize, SurrealValue)]
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
    #[allow(clippy::too_many_arguments)]
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

    /// Get daily usage for the last `days` days, zero-filling missing dates.
    pub async fn query_daily_breakdown(&self, days: usize) -> OpenFangResult<Vec<DailyUsage>> {
        if days == 0 {
            return Ok(Vec::new());
        }

        #[derive(Deserialize, SurrealValue)]
        struct UsageEventRow {
            created_at: String,
            input_tokens: u64,
            output_tokens: u64,
            cost_usd: f64,
        }

        let today = Utc::now().date_naive();
        let start_date = today - chrono::Duration::days(days.saturating_sub(1) as i64);
        let since = start_date
            .and_hms_opt(0, 0, 0)
            .expect("valid midnight")
            .and_utc()
            .to_rfc3339();

        let mut result = self
            .db
            .query(
                "SELECT created_at, input_tokens, output_tokens, cost_usd
                 FROM usage
                 WHERE created_at >= $since
                 ORDER BY created_at ASC",
            )
            .bind(("since", since))
            .await
            .map_err(surreal_err)?;
        let rows: Vec<UsageEventRow> = result.take(0).unwrap_or_default();

        let mut by_date = HashMap::<String, DailyUsage>::new();
        for offset in 0..days {
            let date = (start_date + chrono::Duration::days(offset as i64))
                .format("%Y-%m-%d")
                .to_string();
            by_date.insert(
                date.clone(),
                DailyUsage {
                    date,
                    ..DailyUsage::default()
                },
            );
        }

        for row in rows {
            let date = row.created_at.chars().take(10).collect::<String>();
            if let Some(day) = by_date.get_mut(&date) {
                day.cost_usd += row.cost_usd;
                day.tokens += row.input_tokens + row.output_tokens;
                day.calls += 1;
            }
        }

        let mut days_out = Vec::with_capacity(days);
        for offset in 0..days {
            let date = (start_date + chrono::Duration::days(offset as i64))
                .format("%Y-%m-%d")
                .to_string();
            if let Some(day) = by_date.remove(&date) {
                days_out.push(day);
            }
        }
        Ok(days_out)
    }

    /// Get the timestamp of the earliest recorded usage event.
    pub async fn first_event_date(&self) -> OpenFangResult<Option<String>> {
        #[derive(Deserialize, SurrealValue)]
        struct FirstEventRow {
            created_at: String,
        }

        let mut result = self
            .db
            .query("SELECT created_at FROM usage ORDER BY created_at ASC LIMIT 1")
            .await
            .map_err(surreal_err)?;
        let rows: Vec<FirstEventRow> = result.take(0).unwrap_or_default();
        Ok(rows.into_iter().next().map(|row| row.created_at))
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

        #[derive(Deserialize, SurrealValue)]
        struct Row {
            total: Option<f64>,
        }
        let rows: Vec<Row> = result.take(0).unwrap_or_default();
        Ok(rows.into_iter().next().and_then(|r| r.total).unwrap_or(0.0))
    }
}

/// Usage breakdown per model (returned by `query_by_model`).
#[derive(Debug, Serialize, Deserialize, SurrealValue)]
pub struct ModelUsage {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub call_count: u64,
}

/// Daily usage breakdown for dashboard charts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DailyUsage {
    pub date: String,
    pub cost_usd: f64,
    pub tokens: u64,
    pub calls: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use chrono::{Duration, Utc};

    async fn setup() -> UsageStore {
        let db = db::init_mem().await.unwrap();
        UsageStore::new(db)
    }

    async fn insert_record_at(
        store: &UsageStore,
        agent_id: AgentId,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
        created_at: chrono::DateTime<Utc>,
    ) {
        let _: Option<UsageRecord> = store
            .db
            .create("usage")
            .content(UsageRecord {
                agent_id: agent_id.0.to_string(),
                provider: "test".to_string(),
                model: model.to_string(),
                input_tokens,
                output_tokens,
                cost_usd,
                event_type: "llm_call".to_string(),
                created_at: created_at.to_rfc3339(),
            })
            .await
            .unwrap();
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

    #[tokio::test]
    async fn test_query_by_model_groups_usage() {
        let store = setup().await;
        let agent_a = AgentId::new();
        let agent_b = AgentId::new();

        store
            .record(agent_a, "anthropic", "claude-sonnet", 100, 50, 0.01, "chat")
            .await
            .unwrap();
        store
            .record(agent_b, "anthropic", "claude-sonnet", 200, 75, 0.02, "chat")
            .await
            .unwrap();
        store
            .record(agent_a, "openai", "gpt-4o", 300, 125, 0.03, "chat")
            .await
            .unwrap();

        let grouped = store.query_by_model().await.unwrap();
        let claude = grouped
            .iter()
            .find(|row| row.model == "claude-sonnet")
            .expect("claude-sonnet row should exist");
        let gpt = grouped
            .iter()
            .find(|row| row.model == "gpt-4o")
            .expect("gpt-4o row should exist");

        assert_eq!(claude.input_tokens, 300);
        assert_eq!(claude.output_tokens, 125);
        assert!((claude.cost_usd - 0.03).abs() < 0.001);
        assert_eq!(claude.call_count, 2);

        assert_eq!(gpt.input_tokens, 300);
        assert_eq!(gpt.output_tokens, 125);
        assert!((gpt.cost_usd - 0.03).abs() < 0.001);
        assert_eq!(gpt.call_count, 1);
    }

    #[tokio::test]
    async fn test_query_daily_breakdown_returns_zero_filled_window() {
        let store = setup().await;
        let agent_id = AgentId::new();
        let now = Utc::now()
            .date_naive()
            .and_hms_opt(12, 0, 0)
            .expect("valid noon")
            .and_utc();

        insert_record_at(
            &store,
            agent_id,
            "claude-sonnet",
            100,
            50,
            0.01,
            now - Duration::days(6),
        )
        .await;
        insert_record_at(
            &store,
            agent_id,
            "claude-sonnet",
            200,
            100,
            0.02,
            now - Duration::days(2),
        )
        .await;
        insert_record_at(&store, agent_id, "gpt-4o", 80, 20, 0.005, now).await;

        let days = store.query_daily_breakdown(7).await.unwrap();

        assert_eq!(days.len(), 7);
        assert_eq!(
            days[0].date,
            (now - Duration::days(6)).format("%Y-%m-%d").to_string()
        );
        assert_eq!(days[6].date, now.format("%Y-%m-%d").to_string());

        assert!((days[0].cost_usd - 0.01).abs() < 0.001);
        assert_eq!(days[0].tokens, 150);
        assert_eq!(days[0].calls, 1);

        assert_eq!(days[1].cost_usd, 0.0);
        assert_eq!(days[1].tokens, 0);
        assert_eq!(days[1].calls, 0);

        assert!((days[4].cost_usd - 0.02).abs() < 0.001);
        assert_eq!(days[4].tokens, 300);
        assert_eq!(days[4].calls, 1);

        assert!((days[6].cost_usd - 0.005).abs() < 0.001);
        assert_eq!(days[6].tokens, 100);
        assert_eq!(days[6].calls, 1);
    }

    #[tokio::test]
    async fn test_first_event_date_returns_earliest_usage_timestamp() {
        let store = setup().await;
        let agent_id = AgentId::new();
        let base = Utc::now()
            .date_naive()
            .and_hms_opt(12, 0, 0)
            .expect("valid noon")
            .and_utc();
        let earliest = base - Duration::days(10);
        let latest = base - Duration::days(1);

        insert_record_at(&store, agent_id, "claude-sonnet", 100, 50, 0.01, latest).await;
        insert_record_at(&store, agent_id, "claude-sonnet", 80, 40, 0.008, earliest).await;

        let first = store.first_event_date().await.unwrap();
        let expected = earliest.to_rfc3339();
        assert_eq!(first.as_deref(), Some(expected.as_str()));
    }
}
