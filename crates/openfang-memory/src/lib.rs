//! Memory substrate for the OpenFang Agent Operating System.
//!
//! Provides a unified memory API backed by embedded SurrealDB:
//! - **Structured store**: Key-value pairs, agent state
//! - **Semantic store**: Text + vector search
//! - **Knowledge graph**: Entities and relations (SurrealDB graph edges)
//! - **Session store**: Conversation history + cross-channel canonical sessions
//! - **Usage store**: Token tracking + cost aggregation
//! - **Consolidation engine**: Background decay + pruning
//!
//! Agents interact with a single `Memory` trait that abstracts over all stores.

pub mod consolidation;
pub mod db;
#[cfg(feature = "http-memory")]
pub mod http_client;
pub mod knowledge;
pub mod semantic;
pub mod session;
pub mod structured;
pub mod usage;

mod substrate;
pub use substrate::MemorySubstrate;

/// Code-owned memory surface declarations for the SurrealDB-backed memory crate.
pub fn memory_surface_specs() -> Vec<openfang_types::memory_dimensions::MemorySurfaceSpec> {
    vec![
        semantic::MEMORY_SURFACE,
        structured::KV_SURFACE,
        structured::AGENTS_SURFACE,
        knowledge::KNOWLEDGE_GRAPH_SURFACE,
        session::SESSION_SURFACE,
        usage::USAGE_SURFACE,
        consolidation::CONSOLIDATION_SURFACE,
        substrate::PAIRED_DEVICES_SURFACE,
        substrate::TASK_QUEUE_SURFACE,
        substrate::PROFILE_SURFACE,
    ]
}

#[cfg(test)]
mod dimension_tests {
    use openfang_types::memory_dimensions::MemoryContract;

    #[test]
    fn memory_tables_have_dimension_specs_or_exemptions() {
        let ddl_tables = [
            "sessions",
            "canonical_sessions",
            "memories",
            "kv",
            "agents",
            "usage",
            "entities",
            "relations",
            "profiles",
            "paired_devices",
            "task_queue",
        ];
        let specs = super::memory_surface_specs();

        for table in ddl_tables {
            assert!(
                specs
                    .iter()
                    .any(|spec| spec.storage_tables.contains(&table)),
                "missing memory dimension surface spec for table {table}"
            );
        }
    }

    #[test]
    fn memories_surface_is_multi_model_context_and_operations() {
        let specs = super::memory_surface_specs();
        let memories = specs
            .iter()
            .find(|spec| spec.id == "memories")
            .expect("memories surface spec exists");

        assert!(
            memories.data_models.len() > 1,
            "memories must declare document/vector/full-text data models"
        );
        assert!(memories.contracts.contains(&MemoryContract::Context));
        assert!(memories.contracts.contains(&MemoryContract::Operations));
    }
}
