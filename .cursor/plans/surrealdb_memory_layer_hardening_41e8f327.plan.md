---
name: SurrealDB memory layer hardening
overview: "Audit and harden the SurrealDB memory substrate. The migration is structurally complete but has gaps: zero schema DDL, brute-force vector search, a blocking I/O call in the async hot path, a no-op consolidation, and stale docs."
todos:
  - id: verify-tests
    content: Run cargo test -p openfang-memory and document results (32 tests, all init_mem, zero SQLite refs -- needs confirmation they all pass)
    status: pending
  - id: fix-blocking-mirror
    content: Wrap write_jsonl_mirror() in spawn_blocking to prevent Tokio thread stall
    status: pending
  - id: add-schema-ddl
    content: Add DEFINE TABLE/INDEX/ANALYZER DDL to db::init() for all 8 tables including HNSW on memories.embedding
    status: pending
  - id: wire-consolidation
    content: Wire consolidate() to iterate all agents and call decay/prune/purge on ConsolidationEngine
    status: pending
  - id: multi-hop-graph
    content: Implement max_depth in knowledge.rs query_graph -- currently single-hop only, max_depth param is ignored
    status: pending
  - id: rename-sqlite-path
    content: Rename sqlite_path to db_path in config.rs and kernel.rs
    status: pending
  - id: update-readme
    content: Update README.md feature table from 'SQLite + vector' to 'SurrealDB (multi-model)'
    status: pending
  - id: native-hnsw-search
    content: Replace brute-force Rust cosine re-ranking in semantic.rs with SurrealDB native HNSW KNN query
    status: pending
  - id: agent-serialization
    content: Refactor agent storage from double-serialized JSON string to native SurrealDB document fields
    status: pending
isProject: false
---

## Investigation Results

### What's working well

- SurrealKV embedded storage is initialized and running
- All 6 stores (structured, semantic, knowledge, sessions, usage, consolidation) share one `SurrealDb` handle
- Graph edges use native `RELATE` syntax
- LLM-based session compaction in `openfang-runtime/compactor.rs` is fully async and sophisticated (3-stage: single-pass, chunked, fallback)
- Agent persistence works via JSON-string wrapper pattern (workaround for SurrealDB enum serialization)
- Usage/metering endpoints are wired to async SurrealDB queries
- Zero `rusqlite` references remain in the runtime or memory crates
- 32 memory-layer tests exist, all using `init_mem()` (in-memory SurrealDB), zero SQLite remnants
- `openfang-migrate` is a config-format converter only (OpenClaw JSON5/YAML to TOML) -- zero DB interaction, safe as-is

### Issues found (ordered by priority)

**P0: Blocking I/O in async context** -- [session.rs:461-469](crates/openfang-memory/src/session.rs)
`write_jsonl_mirror()` performs synchronous `std::fs::create_dir_all()`, `File::create()`, `file.write_all()`. If called from an async context (the substrate facade exposes it), this stalls a Tokio worker thread. Fix: wrap in `tokio::task::spawn_blocking`.

**P0: No SurrealDB schema DDL** -- [db.rs:19-34](crates/openfang-memory/src/db.rs)
Zero `DEFINE TABLE`, `DEFINE INDEX`, `DEFINE FIELD`, or `DEFINE ANALYZER` statements exist anywhere. The system relies entirely on SurrealDB's schemaless auto-creation. This means:

- No HNSW vector index on `memories.embedding` (vector search is brute-force in Rust)
- No `DEFINE TABLE entities TYPE NORMAL` or `DEFINE TABLE relations TYPE RELATION` for graph enforcement
- No BM25 full-text index on memory content
- No field type constraints on any table

**Critical prerequisite for HNSW index:** The `DIMENSION` in the DDL must exactly match the embedding model's output. The dimension mapping is at [embedding.rs:106-121](crates/openfang-runtime/src/embedding.rs). The default model per provider is at [kernel.rs:5608-5618](crates/openfang-kernel/src/kernel.rs). Since the embedding model can change at runtime (config or auto-detection), the HNSW index dimension must be resolved at boot time from the active model, not hardcoded. If dimension mismatches, the index silently fails to index vectors.

**P1: `consolidate()` is a no-op** -- [substrate.rs:595-604](crates/openfang-memory/src/substrate.rs)
The `ConsolidationEngine` has working `decay_confidence()`, `prune()`, and `purge_deleted()` methods, but the `Memory::consolidate()` trait implementation returns a zeroed `ConsolidationReport` with the comment "In the future this should iterate over all agents."

**P1: `max_depth` is ignored** -- [knowledge.rs:116](crates/openfang-memory/src/knowledge.rs)
Graph queries are single-hop only. The `GraphPattern.max_depth` field is accepted as a parameter but never referenced in the generated SQL. Collector and Researcher Hands building knowledge graphs can only traverse one hop -- multi-entity queries like "how is X connected to Y through Z" return nothing.

**P1: Stale field name** -- [config.rs:1500](crates/openfang-types/src/config.rs) and [kernel.rs:561](crates/openfang-kernel/src/kernel.rs)
`sqlite_path: Option<PathBuf>` should be `db_path`.

**P1: Stale README** -- [README.md:197](README.md)
Feature comparison table still says "SQLite + vector" for OpenFang's Memory row.

**P2: Vector search is brute-force** -- [semantic.rs:199-222](crates/openfang-memory/src/semantic.rs)
Recall fetches `limit * 10` candidates from SurrealDB using text filters only, then re-ranks by cosine similarity in Rust. No HNSW native query (`<|K,EF|>` operator). The TODO comment at line 5 says "Future: SurrealDB MTREE vector index for native ANN search." Depends on P0 schema DDL being in place first.

**P2: Agent serialization workaround** -- [structured.rs:37-38](crates/openfang-memory/src/structured.rs)
Agents are stored as `{"name": "...", "data": "<json-string>"}` -- the full `AgentEntry` is double-serialized. This blocks using SurrealDB's most powerful features: can't query individual agent fields, can't use `RELATE` on agent properties, can't build indexes on agent attributes. The longer this persists, the more code accumulates around the workaround.

### Priority table


| Priority | Item                                        | Effort  |
| -------- | ------------------------------------------- | ------- |
| Verify   | Run `cargo test -p openfang-memory`         | Trivial |
| P0       | Fix blocking I/O in `write_jsonl_mirror`    | Small   |
| P0       | Add HNSW index + schema DDL to `db::init()` | Medium  |
| P1       | Wire `consolidate()` to iterate agents      | Small   |
| P1       | Implement `max_depth` in graph queries      | Medium  |
| P1       | Rename `sqlite_path` to `db_path`           | Trivial |
| P1       | Update README feature table                 | Trivial |
| P2       | Native HNSW vector search in `semantic.rs`  | Medium  |
| P2       | Agent serialization refactor                | Large   |
| Future   | Live queries, hybrid BM25+vector, SurrealML | Large   |

### Execution order

```
verify-tests -> fix-blocking-mirror -> add-schema-ddl -> wire-consolidation
    -> multi-hop-graph -> rename-sqlite-path + update-readme (batch)
    -> native-hnsw-search -> agent-serialization
```

The P1 cosmetics (`rename-sqlite-path`, `update-readme`) should batch with whatever P1 code change ships next. The P2 items (`native-hnsw-search`, `agent-serialization`) are independent of each other but both depend on the P0 schema DDL being in place.
