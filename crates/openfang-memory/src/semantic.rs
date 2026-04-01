//! Semantic memory store with vector embedding support, backed by SurrealDB.
//!
//! ## Search modes
//!
//! - **Text-only** (no embedding): BM25 full-text search via `@1@` operator against
//!   the `ft_content` index. Better than `CONTAINS` substring matching.
//!
//! - **Vector** (embedding provided, no query text): HNSW KNN via `<|k,ef|>` operator
//!   against the `hnsw_embedding` index. Indexed, sub-linear retrieval.
//!
//! - **Hybrid** (both embedding and query text): Two queries — HNSW KNN and BM25 —
//!   merged in Rust with a 0.6/0.4 weighted score. Deduplicates by record ID.
//!
//! ## Embedding dimension
//!
//! The HNSW index is defined as 768 dimensions to match `nomic-embed-text` (Ollama).
//! All stored embeddings must be 768d. Mismatched records are silently excluded
//! from HNSW results but remain findable via BM25.

use chrono::Utc;
use openfang_types::agent::AgentId;
use openfang_types::error::{OpenFangError, OpenFangResult};
use openfang_types::memory::{MemoryFilter, MemoryFragment, MemoryId, MemorySource};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::debug;

use crate::db::SurrealDb;

/// Semantic store backed by SurrealDB with hybrid vector + full-text search.
#[derive(Clone)]
pub struct SemanticStore {
    db: SurrealDb,
}

/// Memory record as persisted in SurrealDB.
#[derive(Debug, Serialize, Deserialize)]
struct MemoryRecord {
    /// Populated by `meta::id(id) AS record_key` in SELECT queries.
    #[serde(default)]
    record_key: Option<String>,
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

/// Row returned by the HNSW KNN query.
#[derive(Deserialize)]
struct KnnRow {
    record_key: Option<String>,
    agent_id: String,
    content: String,
    source: MemorySource,
    scope: String,
    confidence: f32,
    #[serde(default)]
    metadata: HashMap<String, serde_json::Value>,
    created_at: String,
    accessed_at: String,
    #[serde(default)]
    access_count: u64,
    /// Used for post-filter (we don't push AND conditions into the KNN WHERE clause).
    #[serde(default)]
    deleted: bool,
    embedding: Option<Vec<f32>>,
    /// HNSW cosine distance (lower = closer). Used for ordering; not forwarded to callers.
    #[allow(dead_code)]
    #[serde(default)]
    distance: f64,
}

/// Row returned by the BM25 full-text search query.
#[derive(Deserialize)]
struct BM25Row {
    record_key: Option<String>,
    agent_id: String,
    content: String,
    source: MemorySource,
    scope: String,
    confidence: f32,
    #[serde(default)]
    metadata: HashMap<String, serde_json::Value>,
    created_at: String,
    accessed_at: String,
    #[serde(default)]
    access_count: u64,
    embedding: Option<Vec<f32>>,
    /// BM25 relevance score. Used for rank-based hybrid scoring; not forwarded to callers.
    #[allow(dead_code)]
    #[serde(default)]
    relevance: f64,
}

fn surreal_err(e: surrealdb::Error) -> OpenFangError {
    OpenFangError::Memory(e.to_string())
}

/// Build WHERE clause fragments and bindings from a `MemoryFilter`.
///
/// Returns `(conditions, bindings)` — conditions appended to the caller's list.
fn build_filter_clauses(
    filter: Option<&MemoryFilter>,
    conditions: &mut Vec<String>,
    bindings: &mut Vec<(String, serde_json::Value)>,
) {
    let Some(f) = filter else { return };

    if let Some(agent_id) = f.agent_id {
        conditions.push("agent_id = $filter_aid".to_string());
        bindings.push(("filter_aid".into(), serde_json::json!(agent_id.0.to_string())));
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
        if let Ok(v) = serde_json::to_value(source) {
            conditions.push("source = $filter_source".to_string());
            bindings.push(("filter_source".into(), v));
        }
    }
}

/// Parse a record key string into a `MemoryId`.
///
/// The key was originally written as `id.0.to_string()` (a UUID string), so
/// we attempt a direct UUID parse. Falls back to a fresh ID on failure.
fn parse_memory_id(record_key: Option<&str>) -> MemoryId {
    record_key
        .and_then(|k| uuid::Uuid::parse_str(k).ok())
        .map(MemoryId)
        .unwrap_or_else(MemoryId::new)
}

/// Parse a datetime string into a `chrono::DateTime<Utc>`.
/// Check whether a raw KNN row passes the optional `MemoryFilter` predicates.
fn filter_matches_knn_row(row: &KnnRow, filter: Option<&MemoryFilter>) -> bool {
    let Some(f) = filter else { return true };
    if let Some(aid) = f.agent_id {
        if row.agent_id != aid.0.to_string() {
            return false;
        }
    }
    if let Some(ref scope) = f.scope {
        if &row.scope != scope {
            return false;
        }
    }
    if let Some(min_conf) = f.min_confidence {
        if row.confidence < min_conf {
            return false;
        }
    }
    true
}

fn parse_dt(s: &str) -> chrono::DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
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
                record_key: None,
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

    /// Search for memories.
    ///
    /// Dispatch:
    /// - Both `query` and `query_embedding` present → **hybrid** (HNSW + BM25, merged)
    /// - Only `query_embedding` → **vector** (HNSW KNN)
    /// - Only `query` text → **full-text** (BM25)
    /// - Neither → recent memories by access time
    pub async fn recall_with_embedding(
        &self,
        query: &str,
        limit: usize,
        filter: Option<MemoryFilter>,
        query_embedding: Option<&[f32]>,
    ) -> OpenFangResult<Vec<MemoryFragment>> {
        let has_text = !query.is_empty();
        let has_vec = query_embedding.is_some();

        let fragments = match (has_vec, has_text) {
            (true, true) => {
                self.recall_hybrid(query, limit, filter.as_ref(), query_embedding.unwrap())
                    .await?
            }
            (true, false) => {
                self.recall_vector(limit, filter.as_ref(), query_embedding.unwrap())
                    .await?
            }
            (false, true) => self.recall_text(query, limit, filter.as_ref()).await?,
            (false, false) => self.recall_recent(limit, filter.as_ref()).await?,
        };

        // Update access stats for returned memories in the background (best-effort)
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

    /// HNSW KNN vector search — uses the indexed `<|k,ef|>` operator.
    ///
    /// `ef = 40` gives recall ≥ 0.99 for M=16 (see HNSW research doc).
    ///
    /// The KNN operator is run WITHOUT additional AND conditions because combining
    /// HNSW `<|k,ef|>` with AND-predicate filters in the same WHERE clause can
    /// cause the SurrealDB query planner to short-circuit to 0 results when the
    /// graph is small (< ef candidates). Filters are applied in Rust after fetch.
    /// We over-sample by 3× to account for records filtered out post-fetch.
    async fn recall_vector(
        &self,
        limit: usize,
        filter: Option<&MemoryFilter>,
        query_embedding: &[f32],
    ) -> OpenFangResult<Vec<MemoryFragment>> {
        let oversample = (limit * 3).max(limit + 10);

        let sql = format!(
            "SELECT meta::id(id) AS record_key, agent_id, content, source, scope, confidence,
                    metadata, created_at, accessed_at, access_count, deleted, embedding,
                    vector::distance::knn() AS distance
             FROM memories
             WHERE embedding <|{oversample},40|> $qe
             ORDER BY distance"
        );

        let mut result = self
            .db
            .query(&sql)
            .bind(("qe", query_embedding.to_vec()))
            .await
            .map_err(surreal_err)?;
        let rows: Vec<KnnRow> = result.take(0).unwrap_or_default();

        debug!("Vector recall: {} raw HNSW results, filtering…", rows.len());

        // Post-filter in Rust: deleted flag + MemoryFilter predicates
        let fragments: Vec<MemoryFragment> = rows
            .into_iter()
            .filter(|r| !r.deleted)
            .filter(|r| filter_matches_knn_row(r, filter))
            .take(limit)
            .map(knn_row_to_fragment)
            .collect();

        debug!("Vector recall: {} results after post-filter", fragments.len());
        Ok(fragments)
    }

    /// BM25 full-text search — uses the `@1@` operator against `ft_content`.
    async fn recall_text(
        &self,
        query: &str,
        limit: usize,
        filter: Option<&MemoryFilter>,
    ) -> OpenFangResult<Vec<MemoryFragment>> {
        let mut conditions = vec!["deleted = false".to_string(), "content @1@ $qt".to_string()];
        let mut bindings: Vec<(String, serde_json::Value)> = Vec::new();
        bindings.push(("qt".into(), serde_json::json!(query)));
        build_filter_clauses(filter, &mut conditions, &mut bindings);

        let sql = format!(
            "SELECT meta::id(id) AS record_key, agent_id, content, source, scope, confidence,
                    metadata, created_at, accessed_at, access_count, deleted, embedding,
                    search::score(1) AS relevance
             FROM memories
             WHERE {}
             ORDER BY relevance DESC
             LIMIT {limit}",
            conditions.join(" AND ")
        );

        let mut q = self.db.query(&sql);
        for (k, v) in bindings {
            q = q.bind((k, v));
        }

        let mut result = q.await.map_err(surreal_err)?;
        let rows: Vec<BM25Row> = result.take(0).unwrap_or_default();

        debug!("Text recall: {} results via BM25", rows.len());

        Ok(rows
            .into_iter()
            .map(|r| bm25_row_to_fragment(r))
            .collect())
    }

    /// Hybrid search — runs HNSW KNN and BM25 separately, merges by weighted score.
    ///
    /// Score = 0.6 × (1 − cosine_distance) + 0.4 × normalized_bm25
    async fn recall_hybrid(
        &self,
        query: &str,
        limit: usize,
        filter: Option<&MemoryFilter>,
        query_embedding: &[f32],
    ) -> OpenFangResult<Vec<MemoryFragment>> {
        let oversample = (limit * 3).max(20);

        // Run both queries in parallel
        let (vec_res, txt_res) = tokio::join!(
            self.recall_vector(oversample, filter, query_embedding),
            self.recall_text(query, oversample, filter),
        );

        let vec_frags = vec_res?;
        let txt_frags = txt_res?;

        // Build score map keyed by MemoryId
        let mut scores: HashMap<String, (MemoryFragment, f64)> = HashMap::new();

        // Normalise vector scores: convert cosine distance [0,2] → similarity [0,1]
        let vec_count = vec_frags.len().max(1) as f64;
        for (i, frag) in vec_frags.into_iter().enumerate() {
            // Rank-based score: top result = 1.0, last = ~0
            let vs = 1.0 - (i as f64 / vec_count);
            let key = frag.id.0.to_string();
            scores.insert(key, (frag, vs * 0.6));
        }

        // Normalise BM25 scores by rank
        let txt_count = txt_frags.len().max(1) as f64;
        for (i, frag) in txt_frags.into_iter().enumerate() {
            let ts = 1.0 - (i as f64 / txt_count);
            let key = frag.id.0.to_string();
            scores
                .entry(key)
                .and_modify(|(_, s)| *s += ts * 0.4)
                .or_insert((frag, ts * 0.4));
        }

        let mut merged: Vec<(MemoryFragment, f64)> = scores.into_values().collect();
        merged.sort_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        merged.truncate(limit);

        debug!(
            "Hybrid recall: {} results (vector + BM25 merged)",
            merged.len()
        );

        Ok(merged.into_iter().map(|(f, _)| f).collect())
    }

    /// Fallback: recent memories ordered by last access time.
    async fn recall_recent(
        &self,
        limit: usize,
        filter: Option<&MemoryFilter>,
    ) -> OpenFangResult<Vec<MemoryFragment>> {
        let mut conditions = vec!["deleted = false".to_string()];
        let mut bindings: Vec<(String, serde_json::Value)> = Vec::new();
        build_filter_clauses(filter, &mut conditions, &mut bindings);

        let sql = format!(
            "SELECT meta::id(id) AS record_key, agent_id, content, source, scope, confidence,
                    metadata, created_at, accessed_at, access_count, deleted, embedding
             FROM memories
             WHERE {}
             ORDER BY accessed_at DESC, access_count DESC
             LIMIT {limit}",
            conditions.join(" AND ")
        );

        let mut q = self.db.query(&sql);
        for (k, v) in bindings {
            q = q.bind((k, v));
        }

        let mut result = q.await.map_err(surreal_err)?;
        let records: Vec<MemoryRecord> = result.take(0).unwrap_or_default();

        Ok(records.into_iter().filter_map(memory_record_to_fragment).collect())
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

// ---------------------------------------------------------------------------
// Row → MemoryFragment conversions
// ---------------------------------------------------------------------------

fn memory_record_to_fragment(r: MemoryRecord) -> Option<MemoryFragment> {
    let id = parse_memory_id(r.record_key.as_deref());
    let agent_id_uuid = uuid::Uuid::parse_str(&r.agent_id).ok()?;
    Some(MemoryFragment {
        id,
        agent_id: AgentId(agent_id_uuid),
        content: r.content,
        embedding: r.embedding,
        metadata: r.metadata,
        source: r.source,
        confidence: r.confidence,
        created_at: parse_dt(&r.created_at),
        accessed_at: parse_dt(&r.accessed_at),
        access_count: r.access_count,
        scope: r.scope,
    })
}

fn knn_row_to_fragment(r: KnnRow) -> MemoryFragment {
    let id = parse_memory_id(r.record_key.as_deref());
    let agent_id_uuid = uuid::Uuid::parse_str(&r.agent_id)
        .unwrap_or_else(|_| uuid::Uuid::nil());
    MemoryFragment {
        id,
        agent_id: AgentId(agent_id_uuid),
        content: r.content,
        embedding: r.embedding,
        metadata: r.metadata,
        source: r.source,
        confidence: r.confidence,
        created_at: parse_dt(&r.created_at),
        accessed_at: parse_dt(&r.accessed_at),
        access_count: r.access_count,
        scope: r.scope,
    }
}

fn bm25_row_to_fragment(r: BM25Row) -> MemoryFragment {
    let id = parse_memory_id(r.record_key.as_deref());
    let agent_id_uuid = uuid::Uuid::parse_str(&r.agent_id)
        .unwrap_or_else(|_| uuid::Uuid::nil());
    MemoryFragment {
        id,
        agent_id: AgentId(agent_id_uuid),
        content: r.content,
        embedding: r.embedding,
        metadata: r.metadata,
        source: r.source,
        confidence: r.confidence,
        created_at: parse_dt(&r.created_at),
        accessed_at: parse_dt(&r.accessed_at),
        access_count: r.access_count,
        scope: r.scope,
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
    async fn test_remember_and_recall_text() {
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
                "Memory A about programming",
                MemorySource::Conversation,
                "episodic",
                HashMap::new(),
            )
            .await
            .unwrap();
        store
            .remember(
                AgentId::new(),
                "Memory B about programming",
                MemorySource::Conversation,
                "episodic",
                HashMap::new(),
            )
            .await
            .unwrap();
        let filter = MemoryFilter::agent(agent_id);
        let results = store.recall("programming", 10, Some(filter)).await.unwrap();
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

    #[tokio::test]
    async fn test_record_id_is_stable() {
        // The record ID returned by recall must be stable (not a fresh UUID on
        // each call), so that forget() and update_embedding() work correctly.
        let store = setup().await;
        let agent_id = AgentId::new();
        let written_id = store
            .remember(
                agent_id,
                "Stable identity memory",
                MemorySource::Conversation,
                "facts",
                HashMap::new(),
            )
            .await
            .unwrap();

        let results = store.recall("Stable identity", 10, None).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].id.0, written_id.0,
            "recall() must return the same ID that remember() wrote"
        );
    }

    #[tokio::test]
    async fn test_vector_recall() {
        let store = setup().await;
        let agent_id = AgentId::new();

        // Store two memories with synthetic 384-dimensional embeddings (BGE-small-en-v1.5)
        let emb_a: Vec<f32> = (0..384).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
        let emb_b: Vec<f32> = (0..384).map(|i| if i == 1 { 1.0 } else { 0.0 }).collect();

        store
            .remember_with_embedding(
                agent_id,
                "Memory about Rust async",
                MemorySource::Conversation,
                "episodic",
                HashMap::new(),
                Some(&emb_a),
            )
            .await
            .unwrap();

        store
            .remember_with_embedding(
                agent_id,
                "Memory about Python data science",
                MemorySource::Conversation,
                "episodic",
                HashMap::new(),
                Some(&emb_b),
            )
            .await
            .unwrap();

        // Query with emb_a — should rank the Rust memory first
        let results = store
            .recall_with_embedding("", 2, None, Some(&emb_a))
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert!(
            results[0].content.contains("Rust"),
            "Closest vector should be the Rust memory, got: {}",
            results[0].content
        );
    }

    #[tokio::test]
    async fn test_hybrid_recall() {
        let store = setup().await;
        let agent_id = AgentId::new();

        let emb_rust: Vec<f32> = (0..384).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();

        store
            .remember_with_embedding(
                agent_id,
                "Rust is a systems programming language",
                MemorySource::Conversation,
                "episodic",
                HashMap::new(),
                Some(&emb_rust),
            )
            .await
            .unwrap();

        store
            .remember(
                agent_id,
                "Python is used for data science",
                MemorySource::Conversation,
                "episodic",
                HashMap::new(),
            )
            .await
            .unwrap();

        // Hybrid: vector points to Rust, text matches Rust too
        let results = store
            .recall_with_embedding("Rust programming", 5, None, Some(&emb_rust))
            .await
            .unwrap();

        assert!(!results.is_empty());
        assert!(
            results[0].content.contains("Rust"),
            "Hybrid should rank Rust first, got: {}",
            results[0].content
        );
    }
}
