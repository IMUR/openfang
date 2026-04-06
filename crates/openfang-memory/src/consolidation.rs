//! Memory consolidation engine backed by SurrealDB.
//!
//! Handles background compaction of memory: merging similar fragments,
//! decaying confidence over time, and pruning low-value memories.

use chrono::Utc;
use openfang_types::agent::AgentId;
use openfang_types::error::{OpenFangError, OpenFangResult};
use surrealdb::types::SurrealValue;

use crate::db::SurrealDb;

/// A single episodic memory item selected for L1 summarization.
#[derive(Debug, Clone)]
pub struct EpisodicBatchItem {
    /// SurrealDB record key for this memory (without the table prefix).
    pub memory_id: String,
    /// Text content to be included in the summary prompt.
    pub content: String,
}

/// Consolidation engine backed by SurrealDB.
#[derive(Clone)]
pub struct ConsolidationEngine {
    db: SurrealDb,
}

fn surreal_err(e: surrealdb::Error) -> OpenFangError {
    OpenFangError::Memory(e.to_string())
}

impl ConsolidationEngine {
    /// Create a new consolidation engine wrapping the given SurrealDB handle.
    pub fn new(db: SurrealDb) -> Self {
        Self { db }
    }

    /// Decay confidence for old, unused memories.
    ///
    /// Memories not accessed in `days_threshold` days have their confidence
    /// reduced by `decay_factor` (e.g., 0.95 = 5% decay per run).
    pub async fn decay_confidence(
        &self,
        agent_id: AgentId,
        days_threshold: i64,
        decay_factor: f32,
    ) -> OpenFangResult<u64> {
        let cutoff = (Utc::now() - chrono::Duration::days(days_threshold)).to_rfc3339();

        let mut result = self
            .db
            .query(
                "UPDATE memories SET confidence = confidence * $factor
                 WHERE agent_id = $aid
                   AND deleted = false
                   AND accessed_at < $cutoff
                 RETURN BEFORE",
            )
            .bind(("factor", decay_factor as f64))
            .bind(("aid", agent_id.0.to_string()))
            .bind(("cutoff", cutoff))
            .await
            .map_err(surreal_err)?;

        // Count how many were updated
        let updated: Vec<serde_json::Value> = result.take(0).unwrap_or_default();
        Ok(updated.len() as u64)
    }

    /// Prune memories with confidence below threshold (soft-delete).
    pub async fn prune(&self, agent_id: AgentId, min_confidence: f32) -> OpenFangResult<u64> {
        let mut result = self
            .db
            .query(
                "UPDATE memories SET deleted = true
                 WHERE agent_id = $aid
                   AND deleted = false
                   AND confidence < $min_conf
                 RETURN BEFORE",
            )
            .bind(("aid", agent_id.0.to_string()))
            .bind(("min_conf", min_confidence as f64))
            .await
            .map_err(surreal_err)?;

        let pruned: Vec<serde_json::Value> = result.take(0).unwrap_or_default();
        Ok(pruned.len() as u64)
    }

    /// Return the distinct agent IDs that have at least one non-deleted memory.
    pub async fn all_agent_ids(&self) -> OpenFangResult<Vec<AgentId>> {
        #[derive(serde::Deserialize, SurrealValue)]
        struct Row {
            agent_id: String,
        }

        let mut result = self
            .db
            .query("SELECT DISTINCT agent_id FROM memories WHERE deleted = false")
            .await
            .map_err(surreal_err)?;

        let rows: Vec<Row> = result.take(0).unwrap_or_default();
        let ids = rows
            .into_iter()
            .filter_map(|r| uuid::Uuid::parse_str(&r.agent_id).ok().map(AgentId))
            .collect();
        Ok(ids)
    }

    /// Fetch episodic memories older than `older_than_hours` that have not yet been
    /// included in an L1 summary (no `metadata.summarized_into` key set).
    /// Returns at most `max_items` records ordered by creation time ascending.
    pub async fn fetch_episodic_batch(
        &self,
        agent_id: AgentId,
        older_than_hours: i64,
        max_items: u64,
    ) -> OpenFangResult<Vec<EpisodicBatchItem>> {
        #[derive(serde::Deserialize, SurrealValue)]
        struct Row {
            record_key: Option<String>,
            content: String,
        }

        let cutoff = (Utc::now() - chrono::Duration::hours(older_than_hours)).to_rfc3339();

        let mut result = self
            .db
            .query(
                "SELECT meta::id(id) AS record_key, content
                 FROM memories
                 WHERE agent_id = $aid
                   AND scope = 'episodic'
                   AND deleted = false
                   AND created_at < $cutoff
                   AND (metadata.summarized_into = NONE OR !metadata.summarized_into)
                 ORDER BY created_at ASC
                 LIMIT $lim",
            )
            .bind(("aid", agent_id.0.to_string()))
            .bind(("cutoff", cutoff))
            .bind(("lim", max_items))
            .await
            .map_err(surreal_err)?;

        let rows: Vec<Row> = result.take(0).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                r.record_key.map(|key| EpisodicBatchItem {
                    memory_id: key,
                    content: r.content,
                })
            })
            .collect())
    }

    /// Mark a batch of memories as summarised into `summary_id` and reduce their
    /// confidence to accelerate natural decay (they are no longer the primary source).
    ///
    /// `confidence_reduction` is the fractional amount to subtract (0.3 = 30% reduction).
    pub async fn mark_summarized(
        &self,
        memory_ids: &[String],
        summary_id: &str,
        confidence_reduction: f32,
    ) -> OpenFangResult<()> {
        let factor = (1.0_f64 - confidence_reduction as f64).max(0.0);
        let sid = summary_id.to_string();

        for mid in memory_ids {
            let m = mid.clone();
            let s = sid.clone();
            self.db
                .query(
                    "UPDATE type::record('memories', $mid)
                     SET metadata.summarized_into = $sid,
                         confidence = confidence * $factor",
                )
                .bind(("mid", m))
                .bind(("sid", s))
                .bind(("factor", factor))
                .await
                .map_err(surreal_err)?;
        }
        Ok(())
    }

    /// Hard-delete all soft-deleted memories for an agent.
    pub async fn purge_deleted(&self, agent_id: AgentId) -> OpenFangResult<u64> {
        let mut result = self
            .db
            .query("DELETE memories WHERE agent_id = $aid AND deleted = true RETURN BEFORE")
            .bind(("aid", agent_id.0.to_string()))
            .await
            .map_err(surreal_err)?;

        let purged: Vec<serde_json::Value> = result.take(0).unwrap_or_default();
        Ok(purged.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::semantic::SemanticStore;
    use openfang_types::memory::MemorySource;
    use std::collections::HashMap;

    async fn setup() -> (ConsolidationEngine, SemanticStore) {
        let db = db::init_mem().await.unwrap();
        (ConsolidationEngine::new(db.clone()), SemanticStore::new(db))
    }

    #[tokio::test]
    async fn test_prune_low_confidence() {
        let (engine, store) = setup().await;
        let agent_id = AgentId::new();

        // Store some memories
        store
            .remember(
                agent_id,
                "Important memory",
                MemorySource::UserProvided,
                "facts",
                HashMap::new(),
            )
            .await
            .unwrap();

        // Initially all have confidence 1.0, so prune at 0.5 should remove nothing
        let pruned = engine.prune(agent_id, 0.5).await.unwrap();
        assert_eq!(pruned, 0);
    }
}
