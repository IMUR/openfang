# OpenFang SurrealDB Integration Audit

## Your role

You are auditing the **live, running** OpenFang installation on this machine. Your job is to produce a factual report on the current state of the SurrealDB memory layer — what exists, what's missing, what's misconfigured — based on **observed evidence**, not assumptions from documentation or source code alone.

**Non-destructive only.** You will not CREATE, UPDATE, DELETE, or DEFINE anything. Read-only queries and file inspection only. If you're unsure whether a command mutates state, don't run it.

## Context

OpenFang recently migrated from SQLite to embedded SurrealDB (SurrealKV). The migration is believed to be structurally complete, but the degree of completion has not been independently verified against the running system. Previous sessions documented the codebase but may not reflect the actual deployed state.

Key files:
- Binary config: `~/.openfang/` or wherever `openfang.toml` lives
- SurrealKV data: likely `~/.openfang/data/openfang.db` (check config)
- Source: `~/prj/openfang/` (or locate via `which openfang` / process info)
- Memory crate: `crates/openfang-memory/src/`
- Kernel boot: `crates/openfang-kernel/src/kernel.rs`

## Phase 1: Process and environment

Establish what's actually running before touching the database.

```bash
# Is openfang running? What PID, what port?
ps aux | grep openfang
ss -tlnp | grep -E '4200|4201'

# What binary is running? When was it built?
which openfang
ls -la $(which openfang)
# or check the process binary
ls -la /proc/$(pgrep -f 'openfang')/exe 2>/dev/null

# What config is loaded?
cat ~/.openfang/config.toml 2>/dev/null || cat ~/prj/openfang/openfang.toml 2>/dev/null
# Note: look for the memory section — does it say sqlite_path or db_path?

# Where is the SurrealKV data directory?
find ~/.openfang -name "*.db" -o -name "surrealkv" 2>/dev/null
ls -la ~/.openfang/data/ 2>/dev/null
du -sh ~/.openfang/data/ 2>/dev/null

# What Rust toolchain built this?
cat ~/prj/openfang/rust-toolchain.toml
rustc --version

# What SurrealDB version is in the dependency tree?
cd ~/prj/openfang && cargo tree -p surrealdb 2>/dev/null | head -5
```

Record all findings before proceeding.

## Phase 2: Database introspection via Rust test harness

Since SurrealKV is embedded (no network port), you cannot query it with the `surreal` CLI directly. Instead, write a **read-only** Rust test that connects to the **actual data directory** (not in-memory) and runs introspection queries.

Create a temporary test file (do NOT modify existing tests):

```rust
// File: crates/openfang-memory/tests/audit_readonly.rs
// This test opens the LIVE database in read-only mode and reports state.
// It does NOT write, update, or delete anything.

use surrealdb::engine::local::SurrealKv;
use surrealdb::Surreal;

#[tokio::test]
async fn audit_live_database() -> Result<(), Box<dyn std::error::Error>> {
    // Use the ACTUAL data path from config, not in-memory
    let db_path = std::env::var("OPENFANG_DB_PATH")
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap();
            format!("{home}/.openfang/data/openfang.db")
        });
    
    eprintln!("=== AUDIT: Opening {db_path} ===");
    
    let db = Surreal::new::<SurrealKv>(&db_path).await?;
    db.use_ns("openfang").use_db("agents").await?;

    // 1. What tables exist?
    let tables: surrealdb::Value = db.query("INFO FOR DB").await?.take(0)?;
    eprintln!("=== TABLES ===\n{tables:#}");

    // 2. For each known table, how many records?
    let tables_to_check = [
        "sessions", "canonical_sessions", "memories", "kv", 
        "agents", "usage", "entities", "relations",
        "paired_devices", "task_queue"
    ];
    for table in tables_to_check {
        let count: surrealdb::Value = db
            .query(format!("SELECT count() FROM {table} GROUP ALL"))
            .await?
            .take(0)?;
        eprintln!("  {table}: {count}");
    }

    // 3. What indexes exist on memories?
    let mem_info: surrealdb::Value = db
        .query("INFO FOR TABLE memories")
        .await?
        .take(0)?;
    eprintln!("=== MEMORIES TABLE INFO ===\n{mem_info:#}");

    // 4. What embedding dimensions are actually stored?
    let dims: surrealdb::Value = db
        .query("SELECT array::len(embedding) AS dim FROM memories WHERE embedding IS NOT NONE LIMIT 5")
        .await?
        .take(0)?;
    eprintln!("=== EMBEDDING DIMENSIONS (sample) ===\n{dims:#}");

    // 5. Are there any memories at all? Sample one.
    let sample: surrealdb::Value = db
        .query("SELECT meta::id(id) AS id, agent_id, string::len(content) AS content_len, embedding IS NOT NONE AS has_embedding, confidence, type::of(created_at) AS created_at_type FROM memories LIMIT 3")
        .await?
        .take(0)?;
    eprintln!("=== MEMORY SAMPLES ===\n{sample:#}");

    // 6. Check entities and relations (knowledge graph)
    let ent_sample: surrealdb::Value = db
        .query("SELECT meta::id(id) AS id, entity_type, name FROM entities LIMIT 3")
        .await?
        .take(0)?;
    eprintln!("=== ENTITY SAMPLES ===\n{ent_sample:#}");

    let rel_sample: surrealdb::Value = db
        .query("SELECT meta::id(id) AS id, in, out, relation_type, confidence FROM relations LIMIT 3")
        .await?
        .take(0)?;
    eprintln!("=== RELATION SAMPLES ===\n{rel_sample:#}");

    // 7. Check usage records
    let usage_sample: surrealdb::Value = db
        .query("SELECT agent_id, model, input_tokens, output_tokens, cost_usd FROM usage ORDER BY id DESC LIMIT 3")
        .await?
        .take(0)?;
    eprintln!("=== USAGE SAMPLES ===\n{usage_sample:#}");

    // 8. Check sessions
    let sess_count: surrealdb::Value = db
        .query("SELECT agent_id, array::len(messages) AS msg_count FROM sessions LIMIT 5")
        .await?
        .take(0)?;
    eprintln!("=== SESSION SAMPLES ===\n{sess_count:#}");

    // 9. Agent entries (check for double-serialization)
    let agent_sample: surrealdb::Value = db
        .query("SELECT name, type::of(data) AS data_type, string::len(string(data)) AS data_len FROM agents LIMIT 3")
        .await?
        .take(0)?;
    eprintln!("=== AGENT SAMPLES ===\n{agent_sample:#}");

    // 10. Check for any DEFINE INDEX / ANALYZER already present
    for table in ["memories", "sessions", "entities", "relations", "usage"] {
        let info: surrealdb::Value = db
            .query(format!("INFO FOR TABLE {table}"))
            .await?
            .take(0)?;
        eprintln!("=== INFO FOR {table} ===\n{info:#}");
    }

    eprintln!("=== AUDIT COMPLETE ===");
    Ok(())
}
```

Run it pointing at the live data:

```bash
# IMPORTANT: stop the daemon first to avoid lock contention on SurrealKV
# SurrealKV uses file locks — two processes cannot open the same path
openfang stop  # or systemctl stop openfang

# Run the audit
cd ~/prj/openfang
OPENFANG_DB_PATH="$HOME/.openfang/data/openfang.db" \
  cargo test -p openfang-memory --test audit_readonly -- --nocapture 2>&1 | tee /tmp/openfang-audit.txt

# Restart the daemon
openfang start
```

If stopping the daemon is not acceptable, use an alternative approach — copy the data directory first:

```bash
cp -r ~/.openfang/data /tmp/openfang-audit-copy
OPENFANG_DB_PATH="/tmp/openfang-audit-copy/openfang.db" \
  cargo test -p openfang-memory --test audit_readonly -- --nocapture 2>&1 | tee /tmp/openfang-audit.txt
rm -rf /tmp/openfang-audit-copy
```

## Phase 3: Source code verification

After the database audit, verify the code matches what we observed.

```bash
cd ~/prj/openfang

# 1. Confirm zero SQLite references in memory and runtime crates
grep -rn "rusqlite\|sqlite\|SqliteConnection" crates/openfang-memory/src/ crates/openfang-runtime/src/

# 2. What does db::init() actually do?
cat crates/openfang-memory/src/db.rs

# 3. Any DEFINE statements anywhere in the codebase?
grep -rn "DEFINE TABLE\|DEFINE INDEX\|DEFINE ANALYZER\|DEFINE FIELD" crates/openfang-memory/src/

# 4. How is the embedding dimension determined?
grep -rn "dimension\|DIMENSION\|embed.*dim\|infer_dim" crates/openfang-memory/src/ crates/openfang-runtime/src/

# 5. What does the config struct look like?
grep -n "sqlite_path\|db_path\|surreal" crates/openfang-types/src/config.rs

# 6. How does semantic recall work? (brute-force vs KNN)
sed -n '100,230p' crates/openfang-memory/src/semantic.rs

# 7. What does consolidate() do?
grep -A 15 "fn consolidate" crates/openfang-memory/src/substrate.rs

# 8. What does max_depth do in knowledge queries?
grep -B 5 -A 20 "max_depth" crates/openfang-memory/src/knowledge.rs

# 9. How are agents serialized?
sed -n '30,50p' crates/openfang-memory/src/structured.rs

# 10. Current test count and pass status
cargo test -p openfang-memory 2>&1 | tail -5
```

## Phase 4: Cross-reference and report

Produce a report with the following sections. Every claim must cite its evidence (command output, line number, query result). Do not infer or assume.

### Report structure

```markdown
# OpenFang SurrealDB Integration Audit
Date: [date]
Auditor: [agent]
Binary: [path, build date]
Data path: [path, size on disk]
SurrealDB version: [from cargo tree]

## 1. Database state
- Tables found: [list from INFO FOR DB]
- Tables expected but missing: [any from the 10-table list not found]
- Total records per table: [counts]
- Schema enforcement: [any DEFINE TABLE/FIELD/INDEX present? or fully schemaless]

## 2. Embedding analysis
- Memories with embeddings: [count]
- Memories without embeddings: [count]
- Embedding dimensions observed: [from array::len query — are they consistent?]
- HNSW index present: [yes/no, from INFO FOR TABLE]
- BM25 index present: [yes/no]
- Current recall method: [brute-force in Rust / native KNN — from source]

## 3. Knowledge graph
- Entity count: [number]
- Relation count: [number]
- Relation table type: [TYPE RELATION enforced? or schemaless]
- Max depth support: [implemented / ignored — from source]

## 4. Session state  
- Active sessions: [count]
- Message counts: [sample]
- Compaction status: [any canonical_sessions with compacted data?]

## 5. Agent storage
- Agent count: [number]
- Serialization format: [native document / double-serialized JSON string]
- Data type observed: [from type::of query]

## 6. Usage metering
- Records present: [count]
- Date field type: [string / native datetime]
- Any time-series views defined: [yes/no]

## 7. Configuration
- Config field name: [sqlite_path / db_path]
- Serde alias present: [yes/no]
- README accuracy: [matches actual backend?]

## 8. Gaps and risks
[List every discrepancy between expected state and observed state.
For each gap, note: severity, what breaks, and what fix is needed.]
```

## Rules

1. **Read only.** No CREATE, UPDATE, DELETE, DEFINE, RELATE, or INSERT.
2. **Evidence first.** Every finding must include the command or query that produced it. No "it appears that" or "likely" — say what you observed.
3. **Stop the daemon before opening SurrealKV directly**, or copy the data directory first. SurrealKV file locks will cause errors or corruption if two processes access the same path.
4. **Do not modify existing source files or tests.** The audit test goes in a new file. Clean it up after.
5. **If a query errors**, report the error — that's a finding. Don't skip it.
6. **Compare what you find to the hardening plan** at `.cursor/plans/surrealdb_memory_layer_hardening_41e8f327.plan.md` — does the plan match reality?
