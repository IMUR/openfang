use openfang_memory::MemorySubstrate;
use openfang_memory::db::SurrealDb;
use openfang_memory::usage::UsageStore;
use openfang_types::memory::{Session, SessionId};
use openfang_types::agent::{AgentId, AgentEntry};
use openfang_types::error::{OpenFangError, OpenFangResult};
use std::sync::Arc;

/// A synchronous boundary that satisfies legacy downstream requirements 
/// (like `openfang-api` and `openfang-cli`) by airlocking them securely 
/// ahead of the true asynchronous `MemorySubstrate`.
#[derive(Clone)]
pub struct SyncMemoryFacade {
    pub inner: Arc<MemorySubstrate>,
}

impl SyncMemoryFacade {
    pub fn new(inner: Arc<MemorySubstrate>) -> Self {
        Self { inner }
    }

    /// Extracted SurrealDB database handle, historically used to construct UsageStore.
    /// This seamlessly replaces the missing `usage_conn()` method to stop compilation errors.
    pub fn usage_conn(&self) -> SurrealDb {
        self.inner.db_handle()
    }
    
    pub fn usage(&self) -> &UsageStore {
        self.inner.usage()
    }

    pub fn get_session(&self, id: SessionId) -> OpenFangResult<Option<Session>> {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(self.inner.get_session(id)))
    }

    pub fn save_session(&self, session: &Session) -> OpenFangResult<()> {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(self.inner.save_session(session)))
    }

    pub fn list_kv(&self, agent_id: AgentId) -> OpenFangResult<Vec<(String, serde_json::Value)>> {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(self.inner.list_kv(agent_id)))
    }

    pub fn structured_get(&self, agent_id: AgentId, key: &str) -> OpenFangResult<Option<serde_json::Value>> {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(self.inner.structured_get(agent_id, key)))
    }

    pub fn structured_set(&self, agent_id: AgentId, key: &str, value: serde_json::Value) -> OpenFangResult<()> {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(self.inner.structured_set(agent_id, key, value)))
    }

    pub fn structured_delete(&self, agent_id: AgentId, key: &str) -> OpenFangResult<()> {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(self.inner.structured_delete(agent_id, key)))
    }

    pub fn save_agent(&self, entry: &AgentEntry) -> OpenFangResult<()> {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(self.inner.save_agent(entry)))
    }

    pub fn list_sessions(&self) -> OpenFangResult<Vec<serde_json::Value>> {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(self.inner.list_sessions()))
    }

    pub fn delete_session(&self, id: SessionId) -> OpenFangResult<()> {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(self.inner.delete_session(id)))
    }

    pub fn set_session_label(&self, id: SessionId, label: String) -> OpenFangResult<()> {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(self.inner.set_session_label(id, label)))
    }

    pub fn find_session_by_label(&self, agent_id: AgentId, label: &str) -> OpenFangResult<Option<Session>> {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(self.inner.find_session_by_label(agent_id, label)))
    }
}
