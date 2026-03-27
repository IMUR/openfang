//! Semantic memory store with vector embedding support, backed by SurrealDB.
//!
//! Memories are stored as documents. Embeddings are stored as JSON arrays of f32.
//! Vector similarity search is done in Rust for now (re-ranking after fetch).
//! Future: SurrealDB MTREE vector index for native ANN search.

use chrono::Utc;
use openfang_types::agent::AgentId;
use openfang_types::error::{OpenFangError, OpenFangResult};
use openfang_types::memory::{MemoryFilter, MemoryFragment, MemoryId, MemorySource};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::debug;

use crate::db::SurrealDb;

/// Semantic store backed by SurrealDB with optional vector search.
#[derive(Clone)]
pub struct SemanticStore {
    db: SurrealDb,
}

/// Memory record for SurrealDB persistence.
#[derive(Debug, Serialize, Deserialize)]
struct MemoryRecord {
    agent_id: String,
    content: String,
    source: MemorySource,
    scope: String,
    confidence: f32,
    metadata: HashMap<String, serde_json::Value>,
    created_at: String,
    accessed_at: String,
    access_count: u64,
    deleted: bool,
    embedding: Option<Vec<f32>>,
}

fn surreal_err(e: surrealdb::Error) -> OpenFangError {
    OpenFangError::Memory(e.to_string())
}

impl SemanticStore {
    /// Create a new semantic store wrapping the given SurrealDB handle.
    pub fn new(db: SurrealDb) -> Self {
        Self { db }
    }

    /// Store a new memory fragment (without embedding).
    pub async fn remember(
        &self,
        agent_id: AgentId,
        content: &str,
        source: MemorySource,
        scope: &str,
        metadata: HashMap<String, serde_json::Value>,
    ) -> OpenFangResult<MemoryId> {
        self.remember_with_embedding(agent_id, content, source, scope, metadata, None)
            .await
    }

    /// Store a new memory fragment with an optional embedding vector.
    pub async fn remember_with_embedding(
        &self,
        agent_id: AgentId,
        content: &str,
        source: MemorySource,
        scope: &str,
        metadata: HashMap<String, serde_json::Value>,
        embedding: Option<&[f32]>,
    ) -> OpenFangResult<MemoryId> {
        let id = MemoryId::new();
        let now = Utc::now().to_rfc3339();

        let _: Option<MemoryRecord> = self
            .db
            .create(("memories", id.0.to_string().as_str()))
            .content(MemoryRecord {
                agent_id: agent_id.0.to_string(),
                content: content.to_string(),
                source,
                scope: scope.to_string(),
                confidence: 1.0,
                metadata,
                created_at: now.clone(),
                accessed_at: now,
                access_count: 0,
                deleted: false,
                embedding: embedding.map(|e| e.to_vec()),
            })
            .await
            .map_err(surreal_err)?;

        Ok(id)
    }

    /// Search for memories using text matching (fallback, no embeddings).
    pub async fn recall(
        &self,
        query: &str,
        limit: usize,
        filter: Option<MemoryFilter>,
    ) -> OpenFangResult<Vec<MemoryFragment>> {
        self.recall_with_embedding(query, limit, filter, None).await
    }

    /// Search for memories using vector similarity when a query embedding is provided,
    /// falling back to text content search otherwise.
    pub async fn recall_with_embedding(
        &self,
        query: &str,
        limit: usize,
        filter: Option<MemoryFilter>,
        query_embedding: Option<&[f32]>,
    ) -> OpenFangResult<Vec<MemoryFragment>> {
        let fetch_limit = if query_embedding.is_some() {
            (limit * 10).max(100)
        } else {
            limit
        };

        // Build SurrealQL query with dynamic filters
        let mut conditions = vec!["deleted = false".to_string()];
        let mut bindings: Vec<(String, serde_json::Value)> = Vec::new();

        if query_embedding.is_none() && !query.is_empty() {
            conditions.push("content CONTAINS $query_text".to_string());
            bindings.push(("query_text".into(), serde_json::json!(query)));
        }

        if let Some(ref f) = filter {
            if let Some(agent_id) = f.agent_id {
                conditions.push("agent_id = $filter_aid".to_string());
                bindings.push((
                    "filter_aid".into(),
                    serde_json::json!(agent_id.0.to_string()),
                ));
            }
            if let Some(ref scope) = f.scope {
                conditions.push("scope = $filter_scope".to_string());
                bindings.push(("filter_scope".into(), serde_json::json!(scope)));
            }
            if let Some(min_conf) = f.min_confidence {
                conditions.push("confidence >= $filter_conf".to_string());
                bindings.push(("filter_conf".into(), serde_json::json!(min_conf)));
            }
            if let Some(ref source) = f.source {
                conditions.push("source = $filter_source".to_string());
                bindings.push((
                    "filter_source".into(),
                    serde_json::to_value(source)
                        .map_err(|e| OpenFangError::Serialization(e.to_string()))?,
                ));
            }
        }

        let where_clause = conditions.join(" AND ");
        let sql = format!(
            "SELECT * FROM memories WHERE {where_clause} \
             ORDER BY accessed_at DESC, access_count DESC \
             LIMIT {fetch_limit}"
        );

        let mut query_builder = self.db.query(&sql);
        for (key, val) in bindings {
            query_builder = query_builder.bind((key, val));
        }

        let mut result = query_builder.await.map_err(surreal_err)?;
        let records: Vec<MemoryRecord> = result.take(0).unwrap_or_default();

        let mut fragments: Vec<MemoryFragment> = records
            .into_iter()
            .filter_map(|r| {
                let id = uuid::Uuid::parse_str(&r.agent_id).ok()?;
                let created_at = chrono::DateTime::parse_from_rfc3339(&r.created_at)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                let accessed_at = chrono::DateTime::parse_from_rfc3339(&r.accessed_at)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());

                Some(MemoryFragment {
                    id: MemoryId::new(), // TODO: extract from SurrealDB record ID
                    agent_id: AgentId(id),
                    content: r.content,
                    embedding: r.embedding,
                    metadata: r.metadata,
                    source: r.source,
                    confidence: r.confidence,
                    created_at,
                    accessed_at,
                    access_count: r.access_count,
                    scope: r.scope,
                })
            })
            .collect();

        // If we have a query embedding, re-rank by cosine similarity
        if let Some(qe) = query_embedding {
            fragments.sort_by(|a, b| {
                let sim_a = a
                    .embedding
                    .as_deref()
                    .map(|e| cosine_similarity(qe, e))
                    .unwrap_or(-1.0);
                let sim_b = b
                    .embedding
                    .as_deref()
                    .map(|e| cosine_similarity(qe, e))
                    .unwrap_or(-1.0);
                sim_b
                    .partial_cmp(&sim_a)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            fragments.truncate(limit);
            debug!(
                "Vector recall: {} results from {} candidates",
                fragments.len(),
                fetch_limit
            );
        }

        // Update access counts for returned memories
        for frag in &fragments {
            let _ = self
                .db
                .query(
                    "UPDATE type::thing('memories', $mid) SET access_count += 1, accessed_at = $now",
                )
                .bind(("mid", frag.id.0.to_string()))
                .bind(("now", Utc::now().to_rfc3339()))
                .await;
        }

        Ok(fragments)
    }

    /// Soft-delete a memory fragment.
    pub async fn forget(&self, id: MemoryId) -> OpenFangResult<()> {
        self.db
            .query("UPDATE type::thing('memories', $mid) SET deleted = true")
            .bind(("mid", id.0.to_string()))
            .await
            .map_err(surreal_err)?;
        Ok(())
    }

    /// Update the embedding for an existing memory.
    pub async fn update_embedding(&self, id: MemoryId, embedding: &[f32]) -> OpenFangResult<()> {
        self.db
            .query("UPDATE type::thing('memories', $mid) SET embedding = $emb")
            .bind(("mid", id.0.to_string()))
            .bind(("emb", embedding.to_vec()))
            .await
            .map_err(surreal_err)?;
        Ok(())
    }
}

/// Compute cosine similarity between two vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < f32::EPSILON {
        0.0
    } else {
        dot / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn setup() -> SemanticStore {
        let db = db::init_mem().await.unwrap();
        SemanticStore::new(db)
    }

    #[tokio::test]
    async fn test_remember_and_recall() {
        let store = setup().await;
        let agent_id = AgentId::new();
        store
            .remember(
                agent_id,
                "The user likes Rust programming",
                MemorySource::Conversation,
                "episodic",
                HashMap::new(),
            )
            .await
            .unwrap();
        let results = store.recall("Rust", 10, None).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Rust"));
    }

    #[tokio::test]
    async fn test_recall_with_filter() {
        let store = setup().await;
        let agent_id = AgentId::new();
        store
            .remember(
                agent_id,
                "Memory A",
                MemorySource::Conversation,
                "episodic",
                HashMap::new(),
            )
            .await
            .unwrap();
        store
            .remember(
                AgentId::new(),
                "Memory B",
                MemorySource::Conversation,
                "episodic",
                HashMap::new(),
            )
            .await
            .unwrap();
        let filter = MemoryFilter::agent(agent_id);
        let results = store.recall("Memory", 10, Some(filter)).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_forget() {
        let store = setup().await;
        let agent_id = AgentId::new();
        let id = store
            .remember(
                agent_id,
                "Forgotten memory",
                MemorySource::Conversation,
                "episodic",
                HashMap::new(),
            )
            .await
            .unwrap();
        store.forget(id).await.unwrap();
        let results = store.recall("Forgotten", 10, None).await.unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let c = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &c).abs() < 0.001);
    }
}
