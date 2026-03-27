//! Memory consolidation engine backed by SurrealDB.
//!
//! Handles background compaction of memory: merging similar fragments,
//! decaying confidence over time, and pruning low-value memories.

use chrono::Utc;
use openfang_types::agent::AgentId;
use openfang_types::error::{OpenFangError, OpenFangResult};

use crate::db::SurrealDb;

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
