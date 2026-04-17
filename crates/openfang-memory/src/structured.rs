//! SurrealDB structured store for key-value pairs and agent persistence.
//!
//! Agents are stored as native SurrealDB documents (no msgpack blobs).
//! KV pairs use a composite record ID `kv:{agent_id}:{key}`.

use chrono::Utc;
use openfang_types::agent::{AgentEntry, AgentId};
use openfang_types::error::{OpenFangError, OpenFangResult};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

use crate::db::SurrealDb;

/// Structured store backed by SurrealDB for key-value operations and agent storage.
#[derive(Clone)]
pub struct StructuredStore {
    db: SurrealDb,
}

/// KV record stored in SurrealDB.
#[derive(Debug, Serialize, Deserialize, SurrealValue)]
struct KvRecord {
    agent_id: String,
    key: String,
    value: serde_json::Value,
    version: u64,
    updated_at: String,
}

/// KV listing row (only key + value from SELECT).
#[derive(Debug, Deserialize, SurrealValue)]
struct KvListRow {
    key: String,
    value: serde_json::Value,
}

/// Agent data wrapper for SurrealDB storage.
/// AgentEntry is stored as a JSON string to avoid SurrealDB's
/// serializer choking on Rust enum types (AgentState, etc.).
#[derive(Debug, Serialize, Deserialize, SurrealValue)]
struct AgentWrapper {
    name: String,
    data: String,
}

/// Lightweight agent listing row.
#[allow(dead_code)]
#[derive(Debug, Deserialize, SurrealValue)]
struct AgentListRow {
    id: serde_json::Value,
    name: String,
}

fn surreal_err(e: surrealdb::Error) -> OpenFangError {
    OpenFangError::Memory(e.to_string())
}

impl StructuredStore {
    /// Create a new structured store wrapping the given SurrealDB handle.
    pub fn new(db: SurrealDb) -> Self {
        Self { db }
    }

    /// Get a value from the key-value store.
    pub async fn get(
        &self,
        agent_id: AgentId,
        key: &str,
    ) -> OpenFangResult<Option<serde_json::Value>> {
        let record_id = kv_id(agent_id, key);
        let result: Option<KvRecord> = self
            .db
            .select(("kv", record_id.as_str()))
            .await
            .map_err(surreal_err)?;
        Ok(result.map(|r| r.value))
    }

    /// Set a value in the key-value store.
    pub async fn set(
        &self,
        agent_id: AgentId,
        key: &str,
        value: serde_json::Value,
    ) -> OpenFangResult<()> {
        let record_id = kv_id(agent_id, key);
        let now = Utc::now().to_rfc3339();

        // Upsert: create or update with version bump
        let _: Option<KvRecord> = self
            .db
            .upsert(("kv", record_id.as_str()))
            .content(KvRecord {
                agent_id: agent_id.0.to_string(),
                key: key.to_string(),
                value,
                version: 1, // SurrealDB handles merge; we just set it
                updated_at: now,
            })
            .await
            .map_err(surreal_err)?;

        Ok(())
    }

    /// Delete a value from the key-value store.
    pub async fn delete(&self, agent_id: AgentId, key: &str) -> OpenFangResult<()> {
        let record_id = kv_id(agent_id, key);
        let _: Option<KvRecord> = self
            .db
            .delete(("kv", record_id.as_str()))
            .await
            .map_err(surreal_err)?;
        Ok(())
    }

    /// List all key-value pairs for an agent.
    pub async fn list_kv(
        &self,
        agent_id: AgentId,
    ) -> OpenFangResult<Vec<(String, serde_json::Value)>> {
        let mut result = self
            .db
            .query("SELECT key, value FROM kv WHERE agent_id = $aid ORDER BY key")
            .bind(("aid", agent_id.0.to_string()))
            .await
            .map_err(surreal_err)?;

        let rows: Vec<KvListRow> = result.take(0).map_err(surreal_err)?;
        Ok(rows.into_iter().map(|r| (r.key, r.value)).collect())
    }

    /// Save an agent entry to the database.
    ///
    /// AgentEntry.id is used as the SurrealDB record key and stripped from
    /// the document body to avoid conflicting with SurrealDB's internal `id`.
    pub async fn save_agent(&self, entry: &AgentEntry) -> OpenFangResult<()> {
        let json_str = serde_json::to_string(entry)
            .map_err(|e| OpenFangError::Serialization(e.to_string()))?;
        let _: Option<AgentWrapper> = self
            .db
            .upsert(("agents", entry.id.0.to_string().as_str()))
            .content(AgentWrapper {
                name: entry.name.clone(),
                data: json_str,
            })
            .await
            .map_err(surreal_err)?;
        Ok(())
    }

    /// Load an agent entry from the database.
    pub async fn load_agent(&self, agent_id: AgentId) -> OpenFangResult<Option<AgentEntry>> {
        let result: Option<AgentWrapper> = self
            .db
            .select(("agents", agent_id.0.to_string().as_str()))
            .await
            .map_err(surreal_err)?;

        match result {
            Some(wrapper) => {
                let entry: AgentEntry = serde_json::from_str(&wrapper.data)
                    .map_err(|e| OpenFangError::Serialization(e.to_string()))?;
                Ok(Some(entry))
            }
            None => Ok(None),
        }
    }

    /// Remove an agent from the database.
    pub async fn remove_agent(&self, agent_id: AgentId) -> OpenFangResult<()> {
        let _: Option<AgentWrapper> = self
            .db
            .delete(("agents", agent_id.0.to_string().as_str()))
            .await
            .map_err(surreal_err)?;
        Ok(())
    }

    /// Load all agent entries from the database.
    ///
    /// Deduplicates by name (first occurrence wins).
    pub async fn load_all_agents(&self) -> OpenFangResult<Vec<AgentEntry>> {
        let records: Vec<AgentWrapper> = self.db.select("agents").await.map_err(surreal_err)?;

        let mut agents = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        for wrapper in records {
            let name_lower = wrapper.name.to_lowercase();
            if !seen_names.insert(name_lower) {
                tracing::info!(agent = %wrapper.name, "Skipping duplicate agent name");
                continue;
            }

            match serde_json::from_str::<AgentEntry>(&wrapper.data) {
                Ok(entry) => agents.push(entry),
                Err(e) => {
                    tracing::warn!(agent = %wrapper.name, error = %e, "Failed to deserialize agent");
                }
            }
        }

        Ok(agents)
    }

    /// List all agents in the database (lightweight: id, name only).
    pub async fn list_agents(&self) -> OpenFangResult<Vec<(String, String, String)>> {
        let records: Vec<AgentWrapper> = self.db.select("agents").await.map_err(surreal_err)?;

        let mut result = Vec::new();
        for wrapper in records {
            // Parse just enough to get state
            let state = serde_json::from_str::<serde_json::Value>(&wrapper.data)
                .ok()
                .and_then(|v| v.get("state").map(|s| s.to_string()))
                .unwrap_or_else(|| "\"unknown\"".to_string());
            let id = serde_json::from_str::<serde_json::Value>(&wrapper.data)
                .ok()
                .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(|s| s.to_string()))
                .unwrap_or_default();
            result.push((id, wrapper.name, state));
        }
        Ok(result)
    }
}

/// Build a deterministic KV record ID from agent ID + key.
fn kv_id(agent_id: AgentId, key: &str) -> String {
    format!("{}:{}", agent_id.0, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn setup() -> StructuredStore {
        let db = db::init_mem().await.unwrap();
        StructuredStore::new(db)
    }

    #[tokio::test]
    async fn test_kv_set_get() {
        let store = setup().await;
        let agent_id = AgentId::new();
        store
            .set(agent_id, "test_key", serde_json::json!("test_value"))
            .await
            .unwrap();
        let value = store.get(agent_id, "test_key").await.unwrap();
        assert_eq!(value, Some(serde_json::json!("test_value")));
    }

    #[tokio::test]
    async fn test_kv_get_missing() {
        let store = setup().await;
        let agent_id = AgentId::new();
        let value = store.get(agent_id, "nonexistent").await.unwrap();
        assert!(value.is_none());
    }

    #[tokio::test]
    async fn test_kv_delete() {
        let store = setup().await;
        let agent_id = AgentId::new();
        store
            .set(agent_id, "to_delete", serde_json::json!(42))
            .await
            .unwrap();
        store.delete(agent_id, "to_delete").await.unwrap();
        let value = store.get(agent_id, "to_delete").await.unwrap();
        assert!(value.is_none());
    }

    #[tokio::test]
    async fn test_kv_update() {
        let store = setup().await;
        let agent_id = AgentId::new();
        store
            .set(agent_id, "key", serde_json::json!("v1"))
            .await
            .unwrap();
        store
            .set(agent_id, "key", serde_json::json!("v2"))
            .await
            .unwrap();
        let value = store.get(agent_id, "key").await.unwrap();
        assert_eq!(value, Some(serde_json::json!("v2")));
    }

    #[tokio::test]
    async fn test_kv_list() {
        let store = setup().await;
        let agent_id = AgentId::new();
        store
            .set(agent_id, "b_key", serde_json::json!("b"))
            .await
            .unwrap();
        store
            .set(agent_id, "a_key", serde_json::json!("a"))
            .await
            .unwrap();
        let pairs = store.list_kv(agent_id).await.unwrap();
        assert_eq!(pairs.len(), 2);
        // Should be ordered by key
        assert_eq!(pairs[0].0, "a_key");
        assert_eq!(pairs[1].0, "b_key");
    }

    #[tokio::test]
    async fn test_save_and_load_agent() {
        let store = setup().await;
        let entry = AgentEntry {
            id: AgentId::new(),
            name: "test-agent".to_string(),
            manifest: Default::default(),
            state: openfang_types::agent::AgentState::Running,
            mode: Default::default(),
            created_at: Utc::now(),
            last_active: Utc::now(),
            parent: None,
            children: vec![],
            session_id: openfang_types::agent::SessionId::new(),
            tags: vec!["test".to_string()],
            identity: Default::default(),
            onboarding_completed: false,
            onboarding_completed_at: None,
        };
        store.save_agent(&entry).await.unwrap();
        let loaded = store.load_agent(entry.id).await.unwrap().unwrap();
        assert_eq!(loaded.name, "test-agent");
        assert_eq!(loaded.tags, vec!["test"]);
    }

    #[tokio::test]
    async fn test_remove_agent() {
        let store = setup().await;
        let entry = AgentEntry {
            id: AgentId::new(),
            name: "doomed".to_string(),
            manifest: Default::default(),
            state: openfang_types::agent::AgentState::Running,
            mode: Default::default(),
            created_at: Utc::now(),
            last_active: Utc::now(),
            parent: None,
            children: vec![],
            session_id: openfang_types::agent::SessionId::new(),
            tags: vec![],
            identity: Default::default(),
            onboarding_completed: false,
            onboarding_completed_at: None,
        };
        store.save_agent(&entry).await.unwrap();
        store.remove_agent(entry.id).await.unwrap();
        assert!(store.load_agent(entry.id).await.unwrap().is_none());
    }
}
