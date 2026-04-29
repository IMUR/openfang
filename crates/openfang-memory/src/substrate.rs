//! MemorySubstrate: unified implementation of the `Memory` trait.
//!
//! Composes the structured store, semantic store, knowledge store,
//! session store, usage store, and consolidation engine behind a
//! single async API backed by SurrealDB.

use crate::consolidation::ConsolidationEngine;
use crate::knowledge::KnowledgeStore;
use crate::semantic::SemanticStore;
use crate::session::{Session, SessionStore};
use crate::structured::StructuredStore;
use crate::usage::UsageStore;

use crate::db::{self, SurrealDb};

use async_trait::async_trait;
use openfang_types::agent::{AgentEntry, AgentId, SessionId};
use openfang_types::error::{OpenFangError, OpenFangResult};
use openfang_types::memory::{
    ConsolidationReport, Entity, ExportFormat, GraphMatch, GraphPattern, ImportReport, Memory,
    MemoryFilter, MemoryFragment, MemoryId, MemorySource, Relation,
};
use openfang_types::memory_dimensions::{
    MemoryContract, MemoryDataModel, MemoryIntelligence, MemorySubstrateKind, MemorySurfaceSpec,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const PAIRED_DEVICES_SURFACE: MemorySurfaceSpec = MemorySurfaceSpec {
    id: "paired_devices",
    description: "Operational mobile/device pairing records",
    storage_tables: &["paired_devices"],
    substrates: &[MemorySubstrateKind::SurrealDb],
    data_models: &[MemoryDataModel::Document],
    contracts: &[MemoryContract::Operations],
    intelligence: &[MemoryIntelligence::None],
};

pub const TASK_QUEUE_SURFACE: MemorySurfaceSpec = MemorySurfaceSpec {
    id: "task_queue",
    description: "Operational inter-agent task queue records",
    storage_tables: &["task_queue"],
    substrates: &[MemorySubstrateKind::SurrealDb],
    data_models: &[MemoryDataModel::Document],
    contracts: &[MemoryContract::Operations, MemoryContract::Tool],
    intelligence: &[MemoryIntelligence::None],
};

pub const PROFILE_SURFACE: MemorySurfaceSpec = MemorySurfaceSpec {
    id: "profiles",
    description: "Profile authority records seeded from user context and runtime state",
    storage_tables: &["profiles"],
    substrates: &[MemorySubstrateKind::SurrealDb],
    data_models: &[MemoryDataModel::Document],
    contracts: &[MemoryContract::Authority, MemoryContract::Context],
    intelligence: &[MemoryIntelligence::None],
};

fn surreal_err(e: surrealdb::Error) -> OpenFangError {
    OpenFangError::Memory(e.to_string())
}

macro_rules! surreal_via_json {
    ($t:ty) => {
        impl surrealdb::types::SurrealValue for $t {
            fn kind_of() -> surrealdb::types::Kind {
                surrealdb::types::Kind::Any
            }

            fn into_value(self) -> surrealdb::types::Value {
                let json = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
                surrealdb::types::SurrealValue::into_value(json)
            }

            fn from_value(value: surrealdb::types::Value) -> Result<Self, surrealdb::types::Error> {
                let json = value.into_json_value();
                serde_json::from_value(json)
                    .map_err(|e| surrealdb::types::Error::internal(e.to_string()))
            }
        }
    };
}

/// Paired device record for SurrealDB persistence.
#[derive(Debug, Serialize, Deserialize)]
struct PairedDeviceRecord {
    device_id: String,
    display_name: String,
    platform: String,
    #[serde(deserialize_with = "openfang_types::datetime::deserialize_rfc3339_string")]
    paired_at: String,
    #[serde(deserialize_with = "openfang_types::datetime::deserialize_rfc3339_string")]
    last_seen: String,
    push_token: Option<String>,
}
surreal_via_json!(PairedDeviceRecord);

/// Task queue record for SurrealDB persistence.
#[derive(Debug, Serialize, Deserialize)]
struct TaskRecord {
    title: String,
    description: String,
    status: String,
    priority: i64,
    assigned_to: String,
    created_by: String,
    #[serde(deserialize_with = "openfang_types::datetime::deserialize_rfc3339_string")]
    created_at: String,
    #[serde(
        default,
        deserialize_with = "openfang_types::datetime::deserialize_optional_rfc3339_string"
    )]
    completed_at: Option<String>,
    result: Option<String>,
}
surreal_via_json!(TaskRecord);

#[derive(Debug, Serialize, Deserialize)]
struct ProfileRecord {
    user_name: Option<String>,
    #[serde(deserialize_with = "openfang_types::datetime::deserialize_rfc3339_string")]
    updated_at: String,
}
surreal_via_json!(ProfileRecord);

/// The unified memory substrate. Implements the `Memory` trait by delegating
/// to specialized stores backed by a shared SurrealDB handle.
pub struct MemorySubstrate {
    db: SurrealDb,
    structured: StructuredStore,
    semantic: SemanticStore,
    knowledge: KnowledgeStore,
    sessions: SessionStore,
    consolidation: ConsolidationEngine,
    usage: UsageStore,
    /// Decay factor applied per consolidation run (0 = no decay, 1 = erase immediately).
    /// Sourced from `MemoryConfig::decay_rate`.
    decay_rate: f32,
    /// Minimum confidence below which memories are soft-deleted during consolidation.
    min_confidence: f32,
    /// Days without access before a memory's confidence starts to decay.
    decay_age_days: i64,
}

impl MemorySubstrate {
    /// Open or create a memory substrate at the given database path using defaults.
    pub async fn open(db_path: &Path) -> OpenFangResult<Self> {
        let handle = db::init(db_path).await?;
        Ok(Self::from_handle(handle))
    }

    /// Open with explicit consolidation config sourced from `MemoryConfig`.
    pub async fn open_with_config(
        db_path: &Path,
        decay_rate: f32,
        min_confidence: f32,
        decay_age_days: i64,
    ) -> OpenFangResult<Self> {
        let handle = db::init(db_path).await?;
        Ok(Self::from_handle_with_config(
            handle,
            decay_rate,
            min_confidence,
            decay_age_days,
        ))
    }

    /// Create an in-memory substrate (for testing).
    pub async fn open_in_memory() -> OpenFangResult<Self> {
        let handle = db::init_mem().await?;
        Ok(Self::from_handle(handle))
    }

    /// Construct from an already-initialised SurrealDB handle with default consolidation params.
    fn from_handle(handle: SurrealDb) -> Self {
        Self::from_handle_with_config(handle, 0.05, 0.1, 7)
    }

    /// Construct from an already-initialised SurrealDB handle with explicit consolidation params.
    fn from_handle_with_config(
        handle: SurrealDb,
        decay_rate: f32,
        min_confidence: f32,
        decay_age_days: i64,
    ) -> Self {
        Self {
            db: handle.clone(),
            structured: StructuredStore::new(handle.clone()),
            semantic: SemanticStore::new(handle.clone()),
            knowledge: KnowledgeStore::new(handle.clone()),
            sessions: SessionStore::new(handle.clone()),
            usage: UsageStore::new(handle.clone()),
            consolidation: ConsolidationEngine::new(handle),
            decay_rate,
            min_confidence,
            decay_age_days,
        }
    }

    /// Get a reference to the usage store.
    pub fn usage(&self) -> &UsageStore {
        &self.usage
    }

    /// Read the global user profile name used for bootstrap/profile authority.
    pub async fn global_profile_user_name(&self) -> OpenFangResult<Option<String>> {
        let result: Option<ProfileRecord> = self
            .db
            .select(("profiles", "global"))
            .await
            .map_err(surreal_err)?;
        Ok(result.and_then(|profile| profile.user_name))
    }

    /// Set the global user profile name.
    pub async fn set_global_profile_user_name(&self, user_name: &str) -> OpenFangResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let _: Option<ProfileRecord> = self
            .db
            .upsert(("profiles", "global"))
            .content(ProfileRecord {
                user_name: Some(user_name.to_string()),
                updated_at: now.clone(),
            })
            .await
            .map_err(surreal_err)?;
        self.db
            .query(
                "UPDATE type::record('profiles', 'global')
                 SET updated_at = type::datetime($updated_at)",
            )
            .bind(("updated_at", now))
            .await
            .map_err(surreal_err)?;
        Ok(())
    }

    /// Get the shared SurrealDB handle (for external store construction).
    pub fn db_handle(&self) -> SurrealDb {
        self.db.clone()
    }

    // -----------------------------------------------------------------
    // Agent persistence
    // -----------------------------------------------------------------

    /// Save an agent entry to persistent storage.
    pub async fn save_agent(&self, entry: &AgentEntry) -> OpenFangResult<()> {
        self.structured.save_agent(entry).await
    }

    /// Load an agent entry from persistent storage.
    pub async fn load_agent(&self, agent_id: AgentId) -> OpenFangResult<Option<AgentEntry>> {
        self.structured.load_agent(agent_id).await
    }

    /// Remove an agent from persistent storage and cascade-delete sessions.
    pub async fn remove_agent(&self, agent_id: AgentId) -> OpenFangResult<()> {
        let _ = self.sessions.delete_agent_sessions(agent_id).await;
        self.structured.remove_agent(agent_id).await
    }

    /// Load all agent entries from persistent storage.
    pub async fn load_all_agents(&self) -> OpenFangResult<Vec<AgentEntry>> {
        self.structured.load_all_agents().await
    }

    /// List all saved agents.
    pub async fn list_agents(&self) -> OpenFangResult<Vec<(String, String, String)>> {
        self.structured.list_agents().await
    }

    // -----------------------------------------------------------------
    // Structured KV operations
    // -----------------------------------------------------------------

    /// Get from the structured store.
    pub async fn structured_get(
        &self,
        agent_id: AgentId,
        key: &str,
    ) -> OpenFangResult<Option<serde_json::Value>> {
        self.structured.get(agent_id, key).await
    }

    /// List all KV pairs for an agent.
    pub async fn list_kv(
        &self,
        agent_id: AgentId,
    ) -> OpenFangResult<Vec<(String, serde_json::Value)>> {
        self.structured.list_kv(agent_id).await
    }

    /// Delete a KV entry for an agent.
    pub async fn structured_delete(&self, agent_id: AgentId, key: &str) -> OpenFangResult<()> {
        self.structured.delete(agent_id, key).await
    }

    /// Set in the structured store.
    pub async fn structured_set(
        &self,
        agent_id: AgentId,
        key: &str,
        value: serde_json::Value,
    ) -> OpenFangResult<()> {
        self.structured.set(agent_id, key, value).await
    }

    // -----------------------------------------------------------------
    // Session management
    // -----------------------------------------------------------------

    /// Get a session by ID.
    pub async fn get_session(&self, session_id: SessionId) -> OpenFangResult<Option<Session>> {
        self.sessions.get_session(session_id).await
    }

    /// Save a session.
    pub async fn save_session(&self, session: &Session) -> OpenFangResult<()> {
        self.sessions.save_session(session).await
    }

    /// Save a session (async alias — kept for API compatibility).
    pub async fn save_session_async(&self, session: &Session) -> OpenFangResult<()> {
        self.sessions.save_session(session).await
    }

    /// Create a new empty session for an agent.
    pub async fn create_session(&self, agent_id: AgentId) -> OpenFangResult<Session> {
        self.sessions.create_session(agent_id).await
    }

    /// List all sessions with metadata.
    pub async fn list_sessions(&self) -> OpenFangResult<Vec<serde_json::Value>> {
        self.sessions.list_sessions().await
    }

    /// Delete a session by ID.
    pub async fn delete_session(&self, session_id: SessionId) -> OpenFangResult<()> {
        self.sessions.delete_session(session_id).await
    }

    /// Delete all sessions belonging to an agent.
    pub async fn delete_agent_sessions(&self, agent_id: AgentId) -> OpenFangResult<()> {
        self.sessions.delete_agent_sessions(agent_id).await
    }

    /// Delete the canonical (cross-channel) session for an agent.
    pub async fn delete_canonical_session(&self, agent_id: AgentId) -> OpenFangResult<()> {
        self.sessions.delete_canonical_session(agent_id).await
    }

    /// Set or clear a session label.
    pub async fn set_session_label(
        &self,
        session_id: SessionId,
        label: Option<&str>,
    ) -> OpenFangResult<()> {
        self.sessions.set_session_label(session_id, label).await
    }

    /// Find a session by label for a given agent.
    pub async fn find_session_by_label(
        &self,
        agent_id: AgentId,
        label: &str,
    ) -> OpenFangResult<Option<Session>> {
        self.sessions.find_session_by_label(agent_id, label).await
    }

    /// List all sessions for a specific agent.
    pub async fn list_agent_sessions(
        &self,
        agent_id: AgentId,
    ) -> OpenFangResult<Vec<serde_json::Value>> {
        self.sessions.list_agent_sessions(agent_id).await
    }

    /// Create a new session with an optional label.
    pub async fn create_session_with_label(
        &self,
        agent_id: AgentId,
        label: Option<&str>,
    ) -> OpenFangResult<Session> {
        self.sessions
            .create_session_with_label(agent_id, label)
            .await
    }

    /// Load canonical session context for cross-channel memory.
    pub async fn canonical_context(
        &self,
        agent_id: AgentId,
        window_size: Option<usize>,
    ) -> OpenFangResult<(Option<String>, Vec<openfang_types::message::Message>)> {
        self.sessions.canonical_context(agent_id, window_size).await
    }

    /// Store an LLM-generated summary.
    pub async fn store_llm_summary(
        &self,
        agent_id: AgentId,
        summary: &str,
        kept_messages: Vec<openfang_types::message::Message>,
    ) -> OpenFangResult<()> {
        self.sessions
            .store_llm_summary(agent_id, summary, kept_messages)
            .await
    }

    /// Archive a full transcript snapshot before active session compaction.
    pub async fn archive_transcript(
        &self,
        agent_id: AgentId,
        session_id: SessionId,
        messages: &[openfang_types::message::Message],
        reason: &str,
    ) -> OpenFangResult<String> {
        self.sessions
            .archive_transcript(agent_id, session_id, messages, reason)
            .await
    }

    /// Return the most recent transcript archive for a session.
    pub async fn latest_transcript_archive(
        &self,
        session_id: SessionId,
    ) -> OpenFangResult<Option<Vec<openfang_types::message::Message>>> {
        self.sessions.latest_transcript_archive(session_id).await
    }

    /// Write a human-readable JSONL mirror of a session to disk.
    pub async fn write_jsonl_mirror(
        &self,
        session: Session,
        sessions_dir: PathBuf,
    ) -> Result<(), std::io::Error> {
        self.sessions
            .write_jsonl_mirror(session, sessions_dir)
            .await
    }

    /// Append messages to the agent's canonical session.
    pub async fn append_canonical(
        &self,
        agent_id: AgentId,
        messages: &[openfang_types::message::Message],
        compaction_threshold: Option<usize>,
    ) -> OpenFangResult<()> {
        self.sessions
            .append_canonical(agent_id, messages, compaction_threshold)
            .await?;
        Ok(())
    }

    /// Load all paired devices from the database.
    pub async fn load_paired_devices(&self) -> OpenFangResult<Vec<serde_json::Value>> {
        let mut result = self
            .db
            .query("SELECT * FROM paired_devices")
            .await
            .map_err(surreal_err)?;

        let records: Vec<PairedDeviceRecord> = result.take(0).unwrap_or_default();
        Ok(records
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "device_id": r.device_id,
                    "display_name": r.display_name,
                    "platform": r.platform,
                    "paired_at": r.paired_at,
                    "last_seen": r.last_seen,
                    "push_token": r.push_token,
                })
            })
            .collect())
    }

    /// Save a paired device to the database (insert or replace).
    pub async fn save_paired_device(
        &self,
        device_id: &str,
        display_name: &str,
        platform: &str,
        paired_at: &str,
        last_seen: &str,
        push_token: Option<&str>,
    ) -> OpenFangResult<()> {
        let _: Option<PairedDeviceRecord> = self
            .db
            .upsert(("paired_devices", device_id))
            .content(PairedDeviceRecord {
                device_id: device_id.to_string(),
                display_name: display_name.to_string(),
                platform: platform.to_string(),
                paired_at: paired_at.to_string(),
                last_seen: last_seen.to_string(),
                push_token: push_token.map(|s| s.to_string()),
            })
            .await
            .map_err(surreal_err)?;
        self.db
            .query(
                "UPDATE type::record('paired_devices', $device_id)
                 SET paired_at = type::datetime($paired_at),
                     last_seen = type::datetime($last_seen)",
            )
            .bind(("device_id", device_id.to_string()))
            .bind(("paired_at", paired_at.to_string()))
            .bind(("last_seen", last_seen.to_string()))
            .await
            .map_err(surreal_err)?;
        Ok(())
    }

    /// Remove a paired device from the database.
    pub async fn remove_paired_device(&self, device_id: &str) -> OpenFangResult<()> {
        let _: Option<PairedDeviceRecord> = self
            .db
            .delete(("paired_devices", device_id))
            .await
            .map_err(surreal_err)?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // Embedding-aware memory operations
    // -----------------------------------------------------------------

    /// Store a memory with an embedding vector.
    pub async fn remember_with_embedding(
        &self,
        agent_id: AgentId,
        content: &str,
        source: MemorySource,
        scope: &str,
        metadata: HashMap<String, serde_json::Value>,
        embedding: Option<&[f32]>,
    ) -> OpenFangResult<MemoryId> {
        self.semantic
            .remember_with_embedding(agent_id, content, source, scope, metadata, embedding)
            .await
    }

    /// Recall memories using vector similarity when a query embedding is provided.
    pub async fn recall_with_embedding(
        &self,
        query: &str,
        limit: usize,
        filter: Option<MemoryFilter>,
        query_embedding: Option<&[f32]>,
    ) -> OpenFangResult<Vec<MemoryFragment>> {
        self.semantic
            .recall_with_embedding(query, limit, filter, query_embedding)
            .await
    }

    /// Update the embedding for an existing memory.
    pub async fn update_embedding(&self, id: MemoryId, embedding: &[f32]) -> OpenFangResult<()> {
        self.semantic.update_embedding(id, embedding).await
    }

    /// Set the `entities` key in a memory record's metadata without overwriting other keys.
    pub async fn set_metadata_entities(
        &self,
        memory_id: &str,
        entity_ids: Vec<String>,
    ) -> OpenFangResult<()> {
        self.semantic
            .set_metadata_entities(memory_id, entity_ids)
            .await
    }

    /// Replace the full metadata of an existing memory record.
    pub async fn update_metadata(
        &self,
        memory_id: &str,
        metadata: std::collections::HashMap<String, serde_json::Value>,
    ) -> OpenFangResult<()> {
        self.semantic.update_metadata(memory_id, metadata).await
    }

    /// List non-deleted memory fragments for an agent, paginated.
    ///
    /// Used by the backfill pipeline to iterate over stored memories and
    /// re-apply NER, classification, and metadata enrichment without a
    /// full recall query.
    pub async fn list_fragments(
        &self,
        agent_id: AgentId,
        offset: usize,
        limit: usize,
    ) -> OpenFangResult<Vec<openfang_types::memory::MemoryFragment>> {
        self.semantic.list_fragments(agent_id, offset, limit).await
    }

    /// Inspect non-deleted semantic fragments without mutating access stats.
    #[allow(clippy::too_many_arguments)]
    pub async fn inspect_fragments(
        &self,
        agent_id: AgentId,
        offset: usize,
        limit: usize,
        query: Option<&str>,
        scope: Option<&str>,
        category: Option<&str>,
        classification_source: Option<&str>,
        since: Option<&str>,
        until: Option<&str>,
    ) -> OpenFangResult<Vec<openfang_types::memory::MemoryFragment>> {
        self.semantic
            .inspect_fragments(
                agent_id,
                offset,
                limit,
                query,
                scope,
                category,
                classification_source,
                since,
                until,
            )
            .await
    }

    /// List graph entities for read-only inspection.
    pub async fn list_entities(&self, offset: usize, limit: usize) -> OpenFangResult<Vec<Entity>> {
        self.knowledge.list_entities(offset, limit).await
    }

    /// List graph entities for one agent.
    pub async fn list_entities_for_agent(
        &self,
        agent_id: AgentId,
        offset: usize,
        limit: usize,
    ) -> OpenFangResult<Vec<Entity>> {
        self.knowledge
            .list_entities_for_agent(agent_id, offset, limit)
            .await
    }

    /// List graph relations for read-only inspection.
    pub async fn list_relations(
        &self,
        offset: usize,
        limit: usize,
    ) -> OpenFangResult<Vec<GraphMatch>> {
        self.knowledge.list_relations(offset, limit).await
    }

    /// List graph relations for one agent.
    pub async fn list_relations_for_agent(
        &self,
        agent_id: AgentId,
        offset: usize,
        limit: usize,
    ) -> OpenFangResult<Vec<GraphMatch>> {
        self.knowledge
            .list_relations_for_agent(agent_id, offset, limit)
            .await
    }

    /// Multi-hop graph traversal from a source entity (up to 3 hops).
    ///
    /// Returns all entities reachable from `source_entity_id`, deduplicated,
    /// with their hop depth. Used for graph-boosted recall.
    pub async fn traverse_from_entity(
        &self,
        source_entity_id: &str,
        max_depth: usize,
    ) -> OpenFangResult<Vec<crate::knowledge::TraversalNode>> {
        self.knowledge
            .traverse_from(source_entity_id, max_depth)
            .await
    }

    /// Multi-hop graph traversal from a source entity constrained to one agent.
    pub async fn traverse_from_entity_for_agent(
        &self,
        source_entity_id: &str,
        agent_id: AgentId,
        max_depth: usize,
    ) -> OpenFangResult<Vec<crate::knowledge::TraversalNode>> {
        self.knowledge
            .traverse_from_for_agent(source_entity_id, agent_id, max_depth)
            .await
    }

    /// Async recall_with_embedding (alias — all ops are already async).
    pub async fn recall_with_embedding_async(
        &self,
        query: &str,
        limit: usize,
        filter: Option<MemoryFilter>,
        query_embedding: Option<&[f32]>,
    ) -> OpenFangResult<Vec<MemoryFragment>> {
        self.recall_with_embedding(query, limit, filter, query_embedding)
            .await
    }

    /// Async remember_with_embedding (alias — all ops are already async).
    pub async fn remember_with_embedding_async(
        &self,
        agent_id: AgentId,
        content: &str,
        source: MemorySource,
        scope: &str,
        metadata: HashMap<String, serde_json::Value>,
        embedding: Option<&[f32]>,
    ) -> OpenFangResult<MemoryId> {
        self.remember_with_embedding(agent_id, content, source, scope, metadata, embedding)
            .await
    }

    // -----------------------------------------------------------------
    // Active agent discovery
    // -----------------------------------------------------------------

    /// Return the distinct agent IDs that have at least one non-deleted memory.
    pub async fn all_active_agent_ids(
        &self,
    ) -> OpenFangResult<Vec<openfang_types::agent::AgentId>> {
        self.consolidation.all_agent_ids().await
    }

    // -----------------------------------------------------------------
    // L1 Summarization support
    // -----------------------------------------------------------------

    /// Fetch episodic memories older than `older_than_hours` that have not yet been
    /// folded into an L1 summary. Returns at most `max_items` items ordered by creation
    /// time ascending. Called by the kernel before generating a summary via the LLM.
    pub async fn fetch_episodic_for_summarization(
        &self,
        agent_id: AgentId,
        older_than_hours: i64,
        max_items: u64,
    ) -> OpenFangResult<Vec<crate::consolidation::EpisodicBatchItem>> {
        self.consolidation
            .fetch_episodic_batch(agent_id, older_than_hours, max_items)
            .await
    }

    /// Mark a set of memories as summarised into `summary_id` and reduce their
    /// confidence to accelerate natural decay.
    pub async fn mark_memories_summarized(
        &self,
        memory_ids: &[String],
        summary_id: &str,
        confidence_reduction: f32,
    ) -> OpenFangResult<()> {
        self.consolidation
            .mark_summarized(memory_ids, summary_id, confidence_reduction)
            .await
    }

    /// Write a semantic L1 summary memory.
    ///
    /// `summarized_from_ids` is embedded in the metadata as `summarized_from`.
    pub async fn write_l1_summary(
        &self,
        agent_id: AgentId,
        summary_text: &str,
        summarized_from_ids: Vec<String>,
    ) -> OpenFangResult<MemoryId> {
        let mut meta: HashMap<String, serde_json::Value> = HashMap::new();
        meta.insert(
            "summarized_from".to_string(),
            serde_json::json!(summarized_from_ids),
        );
        meta.insert("source_role".to_string(), serde_json::json!("system"));
        meta.insert("category".to_string(), serde_json::json!("observation"));
        meta.insert(
            "classification_source".to_string(),
            serde_json::json!("consolidation"),
        );

        self.semantic
            .remember(
                agent_id,
                summary_text,
                MemorySource::System,
                openfang_types::memory::scope::SEMANTIC,
                meta,
            )
            .await
    }

    /// Write a semantic memory for an LLM session-compaction summary.
    pub async fn write_session_compaction_summary(
        &self,
        agent_id: AgentId,
        session_id: SessionId,
        summary_text: &str,
        compacted_count: usize,
        chunks_used: u32,
        used_fallback: bool,
        reason: &str,
    ) -> OpenFangResult<MemoryId> {
        let mut meta: HashMap<String, serde_json::Value> = HashMap::new();
        meta.insert(
            "session_id".to_string(),
            serde_json::json!(session_id.0.to_string()),
        );
        meta.insert(
            "summary_type".to_string(),
            serde_json::json!("session_compaction"),
        );
        meta.insert(
            "compacted_count".to_string(),
            serde_json::json!(compacted_count),
        );
        meta.insert("chunks_used".to_string(), serde_json::json!(chunks_used));
        meta.insert(
            "used_fallback".to_string(),
            serde_json::json!(used_fallback),
        );
        meta.insert("compaction_reason".to_string(), serde_json::json!(reason));
        meta.insert("source_role".to_string(), serde_json::json!("system"));
        meta.insert("category".to_string(), serde_json::json!("observation"));
        meta.insert(
            "classification_source".to_string(),
            serde_json::json!("session_compaction"),
        );

        self.semantic
            .remember(
                agent_id,
                summary_text,
                MemorySource::System,
                openfang_types::memory::scope::SEMANTIC,
                meta,
            )
            .await
    }

    /// Post a new task to the shared queue. Returns the task ID.
    pub async fn task_post(
        &self,
        title: &str,
        description: &str,
        assigned_to: Option<&str>,
        created_by: Option<&str>,
    ) -> OpenFangResult<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let _: Option<TaskRecord> = self
            .db
            .create(("task_queue", id.as_str()))
            .content(TaskRecord {
                title: title.to_string(),
                description: description.to_string(),
                status: "pending".to_string(),
                priority: 0,
                assigned_to: assigned_to.unwrap_or("").to_string(),
                created_by: created_by.unwrap_or("").to_string(),
                created_at: now.clone(),
                completed_at: None,
                result: None,
            })
            .await
            .map_err(surreal_err)?;
        self.db
            .query(
                "UPDATE type::record('task_queue', $id)
                 SET created_at = type::datetime($created_at)",
            )
            .bind(("id", id.clone()))
            .bind(("created_at", now))
            .await
            .map_err(surreal_err)?;
        Ok(id)
    }

    /// Claim the next pending task. Returns task JSON or None.
    pub async fn task_claim(&self, agent_id: &str) -> OpenFangResult<Option<serde_json::Value>> {
        let mut result = self
            .db
            .query(
                "SELECT * FROM task_queue
                 WHERE status = 'pending' AND (assigned_to = $aid OR assigned_to = '')
                 ORDER BY priority DESC, created_at ASC
                 LIMIT 1",
            )
            .bind(("aid", agent_id.to_string()))
            .await
            .map_err(surreal_err)?;

        let tasks: Vec<TaskRecord> = result.take(0).unwrap_or_default();
        if let Some(task) = tasks.into_iter().next() {
            // Update status to in_progress — extract ID from created_at as key workaround
            self.db
                .query(
                    "UPDATE task_queue SET status = 'in_progress', assigned_to = $aid
                     WHERE status = 'pending' AND title = $title AND created_at = $cat
                     LIMIT 1",
                )
                .bind(("aid", agent_id.to_string()))
                .bind(("title", task.title.clone()))
                .bind(("cat", task.created_at.clone()))
                .await
                .map_err(surreal_err)?;

            Ok(Some(serde_json::json!({
                "id": task.title,
                "title": task.title,
                "description": task.description,
                "status": "in_progress",
                "assigned_to": if task.assigned_to.is_empty() { agent_id.to_string() } else { task.assigned_to },
                "created_by": task.created_by,
                "created_at": task.created_at,
            })))
        } else {
            Ok(None)
        }
    }

    /// Mark a task as completed with a result string.
    pub async fn task_complete(&self, task_id: &str, result: &str) -> OpenFangResult<()> {
        self.db
            .query(
                "UPDATE type::record('task_queue', $tid)
                 SET status = 'completed', result = $result, completed_at = time::now()",
            )
            .bind(("tid", task_id.to_string()))
            .bind(("result", result.to_string()))
            .await
            .map_err(surreal_err)?;
        Ok(())
    }

    /// List tasks, optionally filtered by status.
    pub async fn task_list(&self, status: Option<&str>) -> OpenFangResult<Vec<serde_json::Value>> {
        let sql = match status {
            Some(_) => "SELECT * FROM task_queue WHERE status = $status ORDER BY created_at DESC",
            None => "SELECT * FROM task_queue ORDER BY created_at DESC",
        };

        let mut query = self.db.query(sql);
        if let Some(s) = status {
            query = query.bind(("status", s.to_string()));
        }

        let mut result = query.await.map_err(surreal_err)?;
        let records: Vec<TaskRecord> = result.take(0).unwrap_or_default();

        Ok(records
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "title": r.title,
                    "description": r.description,
                    "status": r.status,
                    "assigned_to": r.assigned_to,
                    "created_by": r.created_by,
                    "created_at": r.created_at,
                    "completed_at": r.completed_at,
                    "result": r.result,
                })
            })
            .collect())
    }
}

#[async_trait]
impl Memory for MemorySubstrate {
    async fn get(&self, agent_id: AgentId, key: &str) -> OpenFangResult<Option<serde_json::Value>> {
        self.structured.get(agent_id, key).await
    }

    async fn set(
        &self,
        agent_id: AgentId,
        key: &str,
        value: serde_json::Value,
    ) -> OpenFangResult<()> {
        self.structured.set(agent_id, key, value).await
    }

    async fn delete(&self, agent_id: AgentId, key: &str) -> OpenFangResult<()> {
        self.structured.delete(agent_id, key).await
    }

    async fn remember(
        &self,
        agent_id: AgentId,
        content: &str,
        source: MemorySource,
        scope: &str,
        metadata: HashMap<String, serde_json::Value>,
    ) -> OpenFangResult<MemoryId> {
        self.semantic
            .remember(agent_id, content, source, scope, metadata)
            .await
    }

    async fn recall(
        &self,
        query: &str,
        limit: usize,
        filter: Option<MemoryFilter>,
    ) -> OpenFangResult<Vec<MemoryFragment>> {
        self.semantic.recall(query, limit, filter).await
    }

    async fn forget(&self, id: MemoryId) -> OpenFangResult<()> {
        self.semantic.forget(id).await
    }

    async fn update_metadata(
        &self,
        memory_id: &str,
        metadata: HashMap<String, serde_json::Value>,
    ) -> OpenFangResult<()> {
        self.semantic.update_metadata(memory_id, metadata).await
    }

    async fn add_entity(&self, entity: Entity) -> OpenFangResult<String> {
        self.knowledge.add_entity(entity).await
    }

    async fn add_relation(&self, relation: Relation) -> OpenFangResult<String> {
        self.knowledge.add_relation(relation).await
    }

    async fn query_graph(&self, pattern: GraphPattern) -> OpenFangResult<Vec<GraphMatch>> {
        self.knowledge.query_graph(pattern).await
    }

    async fn consolidate(&self) -> OpenFangResult<ConsolidationReport> {
        let start = std::time::Instant::now();

        // Discover all agents that have live memories
        let agent_ids = self.consolidation.all_agent_ids().await?;

        let mut total_decayed: u64 = 0;
        let mut total_pruned: u64 = 0;

        for agent_id in agent_ids {
            // Apply confidence decay to memories not accessed recently
            let decayed = self
                .consolidation
                .decay_confidence(agent_id, self.decay_age_days, 1.0 - self.decay_rate)
                .await
                .unwrap_or(0);

            // Soft-delete memories whose confidence has dropped below the threshold
            let pruned = self
                .consolidation
                .prune(agent_id, self.min_confidence)
                .await
                .unwrap_or(0);

            total_decayed += decayed;
            total_pruned += pruned;
        }

        Ok(ConsolidationReport {
            memories_merged: 0, // merge-by-similarity is Phase 3 (SurrealML)
            memories_decayed: total_decayed + total_pruned,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    async fn export(&self, format: ExportFormat) -> OpenFangResult<Vec<u8>> {
        let _ = format;
        Ok(Vec::new())
    }

    async fn import(&self, _data: &[u8], _format: ExportFormat) -> OpenFangResult<ImportReport> {
        Ok(ImportReport {
            entities_imported: 0,
            relations_imported: 0,
            memories_imported: 0,
            errors: vec!["Import not yet implemented".to_string()],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_substrate_kv() {
        let substrate = MemorySubstrate::open_in_memory().await.unwrap();
        let agent_id = AgentId::new();
        substrate
            .set(agent_id, "key", serde_json::json!("value"))
            .await
            .unwrap();
        let val = substrate.get(agent_id, "key").await.unwrap();
        assert_eq!(val, Some(serde_json::json!("value")));
    }

    #[tokio::test]
    async fn test_global_profile_user_name_roundtrip() {
        let substrate = MemorySubstrate::open_in_memory().await.unwrap();

        assert_eq!(substrate.global_profile_user_name().await.unwrap(), None);
        substrate
            .set_global_profile_user_name("Alice")
            .await
            .unwrap();
        assert_eq!(
            substrate
                .global_profile_user_name()
                .await
                .unwrap()
                .as_deref(),
            Some("Alice")
        );
    }

    #[tokio::test]
    async fn test_session_compaction_summary_writes_semantic_memory() {
        let substrate = MemorySubstrate::open_in_memory().await.unwrap();
        let agent_id = AgentId::new();
        let session_id = SessionId::new();

        substrate
            .write_session_compaction_summary(
                agent_id,
                session_id,
                "The user discussed project goals and constraints.",
                24,
                2,
                false,
                "message_count",
            )
            .await
            .unwrap();

        let fragments = substrate
            .inspect_fragments(
                agent_id,
                0,
                10,
                None,
                Some(openfang_types::memory::scope::SEMANTIC),
                None,
                Some("session_compaction"),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(fragments.len(), 1);
        let metadata = &fragments[0].metadata;
        assert_eq!(
            metadata
                .get("summary_type")
                .and_then(|value| value.as_str()),
            Some("session_compaction")
        );
        assert_eq!(
            metadata
                .get("compacted_count")
                .and_then(|value| value.as_u64()),
            Some(24)
        );
        assert_eq!(
            metadata.get("chunks_used").and_then(|value| value.as_u64()),
            Some(2)
        );
        assert_eq!(
            metadata
                .get("compaction_reason")
                .and_then(|value| value.as_str()),
            Some("message_count")
        );
        assert_eq!(
            metadata.get("session_id").and_then(|value| value.as_str()),
            Some(session_id.0.to_string().as_str())
        );
    }

    #[tokio::test]
    async fn test_transcript_archive_retains_pre_compaction_messages() {
        let substrate = MemorySubstrate::open_in_memory().await.unwrap();
        let agent_id = AgentId::new();
        let session_id = SessionId::new();
        let messages = vec![
            openfang_types::message::Message::user("old user turn"),
            openfang_types::message::Message::assistant("old assistant turn"),
        ];

        substrate
            .archive_transcript(agent_id, session_id, &messages, "message_count")
            .await
            .unwrap();

        let archived = substrate
            .latest_transcript_archive(session_id)
            .await
            .unwrap()
            .expect("archive should exist");
        assert_eq!(archived.len(), 2);
        assert_eq!(archived[0].content.text_content(), "old user turn");
        assert_eq!(archived[1].content.text_content(), "old assistant turn");
    }

    #[tokio::test]
    async fn test_substrate_remember_recall() {
        let substrate = MemorySubstrate::open_in_memory().await.unwrap();
        let agent_id = AgentId::new();
        substrate
            .remember(
                agent_id,
                "Rust is a great language",
                MemorySource::Conversation,
                "episodic",
                HashMap::new(),
            )
            .await
            .unwrap();
        let results = substrate.recall("Rust", 10, None).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_task_post_and_list() {
        let substrate = MemorySubstrate::open_in_memory().await.unwrap();
        let id = substrate
            .task_post(
                "Review code",
                "Check the auth module for issues",
                Some("auditor"),
                Some("orchestrator"),
            )
            .await
            .unwrap();
        assert!(!id.is_empty());

        let tasks = substrate.task_list(Some("pending")).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["title"], "Review code");
        assert_eq!(tasks[0]["assigned_to"], "auditor");
        assert_eq!(tasks[0]["status"], "pending");
    }
}
