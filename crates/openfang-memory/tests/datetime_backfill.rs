use openfang_memory::db;

const RELAX_TIMESTAMP_FIELDS: &[&str] = &[
    "REMOVE FIELD created_at ON memories; DEFINE FIELD created_at ON memories TYPE any",
    "REMOVE FIELD accessed_at ON memories; DEFINE FIELD accessed_at ON memories TYPE any",
    "REMOVE FIELD created_at ON sessions; DEFINE FIELD created_at ON sessions TYPE any",
    "REMOVE FIELD updated_at ON sessions; DEFINE FIELD updated_at ON sessions TYPE any",
    "REMOVE FIELD updated_at ON canonical_sessions; DEFINE FIELD updated_at ON canonical_sessions TYPE any",
    "REMOVE FIELD created_at ON entities; DEFINE FIELD created_at ON entities TYPE any",
    "REMOVE FIELD updated_at ON entities; DEFINE FIELD updated_at ON entities TYPE any",
    "REMOVE FIELD created_at ON relations; DEFINE FIELD created_at ON relations TYPE any",
    "REMOVE FIELD updated_at ON kv; DEFINE FIELD updated_at ON kv TYPE any",
    "REMOVE FIELD created_at ON usage; DEFINE FIELD created_at ON usage TYPE any",
    "REMOVE FIELD paired_at ON paired_devices; DEFINE FIELD paired_at ON paired_devices TYPE any",
    "REMOVE FIELD last_seen ON paired_devices; DEFINE FIELD last_seen ON paired_devices TYPE any",
    "REMOVE FIELD created_at ON task_queue; DEFINE FIELD created_at ON task_queue TYPE any",
    "REMOVE FIELD completed_at ON task_queue; DEFINE FIELD completed_at ON task_queue TYPE any",
];

const BACKFILL_QUERIES: &[&str] = &[
    "UPDATE memories SET created_at = type::datetime(created_at) WHERE type::is_string(created_at)",
    "UPDATE memories SET accessed_at = type::datetime(accessed_at) WHERE type::is_string(accessed_at)",
    "UPDATE sessions SET created_at = type::datetime(created_at) WHERE type::is_string(created_at)",
    "UPDATE sessions SET updated_at = type::datetime(updated_at) WHERE type::is_string(updated_at)",
    "UPDATE canonical_sessions SET updated_at = type::datetime(updated_at) WHERE type::is_string(updated_at)",
    "UPDATE entities SET created_at = type::datetime(created_at) WHERE type::is_string(created_at)",
    "UPDATE entities SET updated_at = type::datetime(updated_at) WHERE type::is_string(updated_at)",
    "UPDATE `relations` SET created_at = type::datetime(created_at) WHERE type::is_string(created_at)",
    "UPDATE kv SET updated_at = type::datetime(updated_at) WHERE type::is_string(updated_at)",
    "UPDATE usage SET created_at = type::datetime(created_at) WHERE type::is_string(created_at)",
    "UPDATE paired_devices SET paired_at = type::datetime(paired_at) WHERE type::is_string(paired_at)",
    "UPDATE paired_devices SET last_seen = type::datetime(last_seen) WHERE type::is_string(last_seen)",
    "UPDATE task_queue SET created_at = type::datetime(created_at) WHERE type::is_string(created_at)",
    "UPDATE task_queue SET completed_at = type::datetime(completed_at) WHERE type::is_string(completed_at)",
];

const STRING_TIMESTAMP_COUNT_QUERIES: &[(&str, &str)] = &[
    (
        "memories.created_at",
        "SELECT count() AS count FROM memories WHERE type::is_string(created_at) GROUP ALL",
    ),
    (
        "memories.accessed_at",
        "SELECT count() AS count FROM memories WHERE type::is_string(accessed_at) GROUP ALL",
    ),
    (
        "sessions.created_at",
        "SELECT count() AS count FROM sessions WHERE type::is_string(created_at) GROUP ALL",
    ),
    (
        "sessions.updated_at",
        "SELECT count() AS count FROM sessions WHERE type::is_string(updated_at) GROUP ALL",
    ),
    (
        "canonical_sessions.updated_at",
        "SELECT count() AS count FROM canonical_sessions WHERE type::is_string(updated_at) GROUP ALL",
    ),
    (
        "entities.created_at",
        "SELECT count() AS count FROM entities WHERE type::is_string(created_at) GROUP ALL",
    ),
    (
        "entities.updated_at",
        "SELECT count() AS count FROM entities WHERE type::is_string(updated_at) GROUP ALL",
    ),
    (
        "relations.created_at",
        "SELECT count() AS count FROM `relations` WHERE type::is_string(created_at) GROUP ALL",
    ),
    (
        "kv.updated_at",
        "SELECT count() AS count FROM kv WHERE type::is_string(updated_at) GROUP ALL",
    ),
    (
        "usage.created_at",
        "SELECT count() AS count FROM usage WHERE type::is_string(created_at) GROUP ALL",
    ),
    (
        "paired_devices.paired_at",
        "SELECT count() AS count FROM paired_devices WHERE type::is_string(paired_at) GROUP ALL",
    ),
    (
        "paired_devices.last_seen",
        "SELECT count() AS count FROM paired_devices WHERE type::is_string(last_seen) GROUP ALL",
    ),
    (
        "task_queue.created_at",
        "SELECT count() AS count FROM task_queue WHERE type::is_string(created_at) GROUP ALL",
    ),
    (
        "task_queue.completed_at",
        "SELECT count() AS count FROM task_queue WHERE type::is_string(completed_at) GROUP ALL",
    ),
];

async fn run_datetime_backfill(db: &openfang_memory::db::SurrealDb) {
    for query in RELAX_TIMESTAMP_FIELDS {
        db.query(*query).await.unwrap_or_else(|e| {
            panic!("datetime field relaxation query failed: {query}\n{e}");
        });
    }
    for query in BACKFILL_QUERIES {
        db.query(*query).await.unwrap_or_else(|e| {
            panic!("datetime backfill query failed: {query}\n{e}");
        });
    }
}

async fn assert_no_string_timestamps(db: &openfang_memory::db::SurrealDb) {
    for (label, query) in STRING_TIMESTAMP_COUNT_QUERIES {
        let mut result = db.query(*query).await.unwrap_or_else(|e| {
            panic!("string timestamp count query failed for {label}: {query}\n{e}");
        });
        let rows: Vec<serde_json::Value> = result.take(0).unwrap_or_default();
        let count = rows
            .first()
            .and_then(|row| row.get("count"))
            .and_then(|count| count.as_u64())
            .unwrap_or(0);
        assert_eq!(count, 0, "{label} still has {count} string timestamp(s)");
    }
}

#[tokio::test]
async fn datetime_backfill_queries_convert_strings() {
    let db = db::init_mem().await.unwrap();
    db.query(
        "CREATE memories:test SET
            agent_id = 'agent',
            content = 'hello',
            source = 'conversation',
            scope = 'episodic',
            confidence = 1.0,
            metadata = {},
            created_at = '2026-04-26T10:00:00Z',
            accessed_at = '2026-04-26T10:00:00Z',
            access_count = 0,
            deleted = false",
    )
    .await
    .unwrap();

    run_datetime_backfill(&db).await;

    let mut result = db
        .query(
            "SELECT
                type::is_datetime(created_at) AS created_is_datetime,
                type::is_datetime(accessed_at) AS accessed_is_datetime
             FROM memories WHERE meta::id(id) = 'test'",
        )
        .await
        .unwrap();
    let rows: Vec<serde_json::Value> = result.take(0).unwrap();
    assert_eq!(rows[0]["created_is_datetime"], true);
    assert_eq!(rows[0]["accessed_is_datetime"], true);
    assert_no_string_timestamps(&db).await;
}

#[tokio::test]
#[ignore = "requires stopped daemon and OPENFANG_BACKFILL_DB=/home/prtr/.openfang/data/openfang.db"]
async fn datetime_backfill_live_database() {
    let db_path = std::env::var("OPENFANG_BACKFILL_DB")
        .expect("set OPENFANG_BACKFILL_DB to the stopped live database path");
    let db = db::init(std::path::Path::new(&db_path)).await.unwrap();
    run_datetime_backfill(&db).await;
    assert_no_string_timestamps(&db).await;
}
