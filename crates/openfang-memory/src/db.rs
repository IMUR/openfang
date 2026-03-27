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
    async fn test_init_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = init(dir.path()).await.unwrap();
        let mut result = db.query("RETURN 'alive'").await.unwrap();
        let val: Option<String> = result.take(0).unwrap();
        assert_eq!(val.as_deref(), Some("alive"));
    }
}
