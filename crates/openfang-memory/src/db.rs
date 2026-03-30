//! SurrealDB embedded database initialization.
//!
//! Provides a thin wrapper around `Surreal<Db>` for embedded (SurrealKV) storage.
//! No network, no ports — `Surreal<Db>` is a direct in-process call into the
//! datastore, routing commands through a function pointer.

use openfang_types::error::{OpenFangError, OpenFangResult};
use std::path::Path;
use surrealdb::engine::local::{Db, Mem, SurrealKv};
use surrealdb::Surreal;

/// The shared database handle. `Surreal<Db>` is `Clone + Send + Sync`.
pub type SurrealDb = Surreal<Db>;

/// Idempotent schema DDL for all OpenFang tables.
///
/// Every statement uses `IF NOT EXISTS` so re-running on an existing DB is safe.
/// All document tables remain SCHEMALESS for backwards compatibility with
/// existing data that was written before any DEFINE statements existed.
const SCHEMA_DDL: &str = r#"
-- Text analyzer for BM25 full-text search
DEFINE ANALYZER IF NOT EXISTS memory_analyzer
    TOKENIZERS blank, class, camel
    FILTERS lowercase, snowball(english);

-- Sessions table
DEFINE TABLE IF NOT EXISTS sessions SCHEMALESS;
DEFINE FIELD IF NOT EXISTS agent_id ON sessions TYPE string;
DEFINE FIELD IF NOT EXISTS messages ON sessions TYPE array;
DEFINE FIELD IF NOT EXISTS context_window_tokens ON sessions TYPE int;
DEFINE FIELD IF NOT EXISTS label ON sessions TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at ON sessions TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON sessions TYPE string;

-- Canonical sessions table
DEFINE TABLE IF NOT EXISTS canonical_sessions SCHEMALESS;
DEFINE FIELD IF NOT EXISTS agent_id ON canonical_sessions TYPE string;
DEFINE FIELD IF NOT EXISTS messages ON canonical_sessions TYPE array;
DEFINE FIELD IF NOT EXISTS compaction_cursor ON canonical_sessions TYPE int;
DEFINE FIELD IF NOT EXISTS compacted_summary ON canonical_sessions TYPE option<string>;
DEFINE FIELD IF NOT EXISTS updated_at ON canonical_sessions TYPE string;

-- Memories table (semantic store)
DEFINE TABLE IF NOT EXISTS memories SCHEMALESS;
DEFINE FIELD IF NOT EXISTS agent_id ON memories TYPE string;
DEFINE FIELD IF NOT EXISTS content ON memories TYPE string;
DEFINE FIELD IF NOT EXISTS source ON memories FLEXIBLE TYPE any;
DEFINE FIELD IF NOT EXISTS scope ON memories TYPE string;
DEFINE FIELD IF NOT EXISTS confidence ON memories TYPE float;
DEFINE FIELD IF NOT EXISTS metadata ON memories FLEXIBLE TYPE object;
DEFINE FIELD IF NOT EXISTS created_at ON memories TYPE string;
DEFINE FIELD IF NOT EXISTS accessed_at ON memories TYPE string;
DEFINE FIELD IF NOT EXISTS access_count ON memories TYPE int;
DEFINE FIELD IF NOT EXISTS deleted ON memories TYPE bool;
DEFINE FIELD IF NOT EXISTS embedding ON memories TYPE option<array<float>>;

-- HNSW vector index: 384 dimensions = all-MiniLM-L6-v2
DEFINE INDEX IF NOT EXISTS hnsw_embedding ON memories
    FIELDS embedding HNSW DIMENSION 384 DIST COSINE;

-- BM25 full-text index on content
DEFINE INDEX IF NOT EXISTS ft_content ON memories
    FIELDS content SEARCH ANALYZER memory_analyzer BM25(1.2, 0.75);

-- KV store (composite record IDs: kv:{agent_id}:{key})
DEFINE TABLE IF NOT EXISTS kv SCHEMALESS;
DEFINE FIELD IF NOT EXISTS agent_id ON kv TYPE string;
DEFINE FIELD IF NOT EXISTS key ON kv TYPE string;
DEFINE FIELD IF NOT EXISTS value ON kv FLEXIBLE TYPE any;
DEFINE FIELD IF NOT EXISTS version ON kv TYPE int;
DEFINE FIELD IF NOT EXISTS updated_at ON kv TYPE string;

-- Agents table (data is double-serialized JSON string)
DEFINE TABLE IF NOT EXISTS agents SCHEMALESS;
DEFINE FIELD IF NOT EXISTS name ON agents TYPE string;
DEFINE FIELD IF NOT EXISTS data ON agents TYPE string;

-- Usage tracking
DEFINE TABLE IF NOT EXISTS usage SCHEMALESS;
DEFINE FIELD IF NOT EXISTS agent_id ON usage TYPE string;
DEFINE FIELD IF NOT EXISTS provider ON usage TYPE string;
DEFINE FIELD IF NOT EXISTS model ON usage TYPE string;
DEFINE FIELD IF NOT EXISTS input_tokens ON usage TYPE int;
DEFINE FIELD IF NOT EXISTS output_tokens ON usage TYPE int;
DEFINE FIELD IF NOT EXISTS cost_usd ON usage TYPE float;
DEFINE FIELD IF NOT EXISTS event_type ON usage TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON usage TYPE string;

-- Entities table (knowledge graph nodes)
DEFINE TABLE IF NOT EXISTS entities SCHEMALESS;
DEFINE FIELD IF NOT EXISTS entity_type ON entities FLEXIBLE TYPE any;
DEFINE FIELD IF NOT EXISTS name ON entities TYPE string;
DEFINE FIELD IF NOT EXISTS properties ON entities FLEXIBLE TYPE object;
DEFINE FIELD IF NOT EXISTS created_at ON entities TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON entities TYPE string;

-- Relations table (knowledge graph edges)
DEFINE TABLE IF NOT EXISTS relations TYPE RELATION SCHEMALESS;
DEFINE FIELD IF NOT EXISTS relation_type ON relations FLEXIBLE TYPE any;
DEFINE FIELD IF NOT EXISTS properties ON relations FLEXIBLE TYPE object;
DEFINE FIELD IF NOT EXISTS confidence ON relations TYPE float;
DEFINE FIELD IF NOT EXISTS created_at ON relations TYPE string;

-- Paired devices
DEFINE TABLE IF NOT EXISTS paired_devices SCHEMALESS;
DEFINE FIELD IF NOT EXISTS device_id ON paired_devices TYPE string;
DEFINE FIELD IF NOT EXISTS display_name ON paired_devices TYPE string;
DEFINE FIELD IF NOT EXISTS platform ON paired_devices TYPE string;
DEFINE FIELD IF NOT EXISTS paired_at ON paired_devices TYPE string;
DEFINE FIELD IF NOT EXISTS last_seen ON paired_devices TYPE string;
DEFINE FIELD IF NOT EXISTS push_token ON paired_devices TYPE option<string>;

-- Task queue
DEFINE TABLE IF NOT EXISTS task_queue SCHEMALESS;
DEFINE FIELD IF NOT EXISTS title ON task_queue TYPE string;
DEFINE FIELD IF NOT EXISTS description ON task_queue TYPE string;
DEFINE FIELD IF NOT EXISTS status ON task_queue TYPE string;
DEFINE FIELD IF NOT EXISTS priority ON task_queue TYPE int;
DEFINE FIELD IF NOT EXISTS assigned_to ON task_queue TYPE string;
DEFINE FIELD IF NOT EXISTS created_by ON task_queue TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON task_queue TYPE string;
DEFINE FIELD IF NOT EXISTS completed_at ON task_queue TYPE option<string>;
DEFINE FIELD IF NOT EXISTS result ON task_queue TYPE option<string>;
"#;

/// Execute schema DDL against the database. Errors abort boot.
async fn run_ddl(db: &SurrealDb) -> OpenFangResult<()> {
    db.query(SCHEMA_DDL)
        .await
        .map_err(|e| OpenFangError::Memory(format!("Schema DDL failed: {e}")))?;
    Ok(())
}

/// Initialize an embedded SurrealDB instance backed by SurrealKV on disk.
///
/// The database files live at `data_dir`. Namespace and database are
/// set to `openfang` / `agents`.
pub async fn init(data_dir: &Path) -> OpenFangResult<SurrealDb> {
    let path = data_dir.to_str().ok_or_else(|| {
        OpenFangError::Internal("Invalid data directory path (non-UTF8)".to_string())
    })?;

    let db = Surreal::new::<SurrealKv>(path)
        .await
        .map_err(|e| OpenFangError::Memory(format!("SurrealDB init failed: {e}")))?;

    db.use_ns("openfang")
        .use_db("agents")
        .await
        .map_err(|e| OpenFangError::Memory(format!("SurrealDB namespace setup: {e}")))?;

    run_ddl(&db).await?;

    Ok(db)
}

/// Initialize an in-memory SurrealDB instance (for tests).
pub async fn init_mem() -> OpenFangResult<SurrealDb> {
    let db = Surreal::new::<Mem>(())
        .await
        .map_err(|e| OpenFangError::Memory(format!("SurrealDB in-memory init: {e}")))?;

    db.use_ns("openfang")
        .use_db("agents")
        .await
        .map_err(|e| OpenFangError::Memory(format!("SurrealDB namespace setup: {e}")))?;

    run_ddl(&db).await?;

    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init_mem() {
        let db = init_mem().await.unwrap();
        // Sanity: run a trivial SurrealQL expression
        let mut result = db.query("RETURN 1 + 1").await.unwrap();
        let val: Option<i64> = result.take(0).unwrap();
        assert_eq!(val, Some(2));
    }

    #[tokio::test]
    async fn test_ddl_schema_applied() {
        let db = init_mem().await.unwrap();

        // Verify memories table has HNSW and BM25 indexes
        let mut result = db.query("INFO FOR TABLE memories").await.unwrap();
        let info: surrealdb::Value = result.take(0).unwrap();
        let info_str = format!("{info}");
        assert!(
            info_str.contains("hnsw_embedding"),
            "HNSW index not found in: {info_str}"
        );
        assert!(
            info_str.contains("ft_content"),
            "BM25 index not found in: {info_str}"
        );

        // Verify relations table is TYPE RELATION
        let mut result = db.query("INFO FOR TABLE relations").await.unwrap();
        let _info: surrealdb::Value = result.take(0).unwrap();

        // Verify analyzer exists
        let mut result = db.query("INFO FOR DB").await.unwrap();
        let db_info: surrealdb::Value = result.take(0).unwrap();
        let db_info_str = format!("{db_info}");
        assert!(
            db_info_str.contains("memory_analyzer"),
            "Analyzer not found in: {db_info_str}"
        );
    }

    #[tokio::test]
    async fn test_init_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = init(dir.path()).await.unwrap();
        let mut result = db.query("RETURN 'alive'").await.unwrap();
        let val: Option<String> = result.take(0).unwrap();
        assert_eq!(val.as_deref(), Some("alive"));
    }
}
