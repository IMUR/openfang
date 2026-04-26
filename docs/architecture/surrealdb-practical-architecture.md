# SurrealDB in OpenFang: A Practical Breakdown

**Date:** 2026-04-25
**Branch:** `main`
**Scope:** Functional architecture of the SurrealDB substrate, what is currently used, what is not, and where the system can go from an application-level perspective.

---

## What SurrealDB Actually Is (The Uncommon Mental Model)

Most descriptions file SurrealDB as "a document database with extras." That undersells it in ways that matter practically. The better mental model is: **SurrealDB is a unified query surface over five completely different data models — document, key-value, graph, vector, and full-text — that share a single storage engine, a single query language (SurrealQL), and a single in-process handle.**

For an agent operating system this is not a convenience feature. It means the same piece of state — a memory fragment — can be a document with arbitrary metadata, a node in a knowledge graph, the target of a vector KNN query, and a full-text BM25 match, all simultaneously, without joining across systems. OpenFang leverages this in the `memories` table, which is the most important table in the whole substrate.

SurrealKV, the storage engine underneath, is an LSM-tree backed key-value store. All five models live on top of it. There is no separate Postgres for relational, no Redis for KV, no Qdrant for vectors — one file tree at `~/.openfang/data/openfang.db/`.

---

## What OpenFang Currently Uses: The Active Surface

### 1. Document Store — The Universal Container

Every table in OpenFang is `SCHEMALESS` with optional `DEFINE FIELD` hints. This combination is deliberate:

- `SCHEMALESS` means old records with missing fields don't break new code that reads them.
- `DEFINE FIELD` hints tell the query planner about type expectations and enable index creation.

In practice, `memories`, `sessions`, `canonical_sessions`, `kv`, `agents`, `usage`, `entities`, `paired_devices`, and `task_queue` are all plain documents. The document store is doing by far the most work.

The `agents` table has an important quirk worth understanding: `AgentEntry` is **double-serialized** — the entire Rust struct is converted to a JSON string and stored in a `data: string` field. This was done to avoid SurrealDB's serializer choking on Rust enum variants like `AgentState`. It works but it makes the `agents` table opaque — you can't `SELECT data.model FROM agents` in SurrealQL because `data` is a string blob, not an embedded object.

### 2. HNSW Vector Index — The Primary Recall Engine

The `memories` table has:

```sql
DEFINE INDEX hnsw_embedding ON memories
    FIELDS embedding HNSW DIMENSION 384 DIST COSINE M 16 EFC 200 TYPE F32;
```

This is a Hierarchical Navigable Small World graph embedded directly in the SurrealKV layer. The `M=16 EFC=200` parameters mean each node connects to 16 candidates during construction (`M`), and during recall searches 200 candidate nodes per layer (`EFC`). With `ef=40` at query time (what OpenFang uses), recall is ≥ 0.99 for this graph size — essentially all true nearest neighbours are found.

What's practically important: the HNSW index is rebuilt from scratch on every boot (`REMOVE INDEX IF EXISTS` then `DEFINE`). This means dimension migration is safe (change the number, restart, old embeddings just stop being indexed until their records are updated), but it means every new boot takes whatever time it takes to rebuild the graph from all stored embeddings. For a large `memories` table this will eventually become noticeable.

The KNN query syntax in SurrealDB is unusual:

```sql
WHERE embedding <|oversample,ef|> $query_vector
```

The `<|k,ef|>` operator is the HNSW approximate nearest-neighbour search. OpenFang oversamples 3× and post-filters in Rust because pushing `AND agent_id = $aid` into the same WHERE clause as the KNN operator confuses the SurrealDB 3.x query planner — it can short-circuit to zero results when the HNSW graph is small relative to `ef`. This is a known SurrealDB behaviour, not an OpenFang bug.

### 3. BM25 Full-Text Index — The Fallback and Complement

```sql
DEFINE ANALYZER IF NOT EXISTS memory_analyzer
    TOKENIZERS blank, class, camel
    FILTERS lowercase, snowball(english);

DEFINE INDEX IF NOT EXISTS ft_content ON memories
    FIELDS content FULLTEXT ANALYZER memory_analyzer BM25 HIGHLIGHTS;
```

The `memory_analyzer` tokenizes on whitespace (`blank`), character class boundaries (`class`), and camelCase word boundaries (`camel`). English stemming via snowball means "running", "runs", "ran" all map to the same term. The BM25 scoring function ranks by term frequency × inverse document frequency.

The `@1@` query operator is SurrealDB's BM25 search syntax:

```sql
content @1@ $query_text
```

The `1` is the analyzer index reference (matching the `BM25` index definition order). `search::score(1)` retrieves the BM25 relevance score.

This index has `HIGHLIGHTS` defined, which enables `search::highlight('<b>', '</b>', 1)` in queries — a feature OpenFang doesn't currently use but which could power search result snippets in a future dashboard or tool.

### 4. Graph Database — The Knowledge Layer

The `entities` and `relations` tables form a proper property graph:

```sql
DEFINE TABLE IF NOT EXISTS relations TYPE RELATION SCHEMALESS;
```

`TYPE RELATION` is what makes `relations` a first-class edge table. In SurrealDB, edges are records too — they have their own document fields (`relation_type`, `properties`, `confidence`) while also having SurrealDB-managed `in` (source) and `out` (target) record links.

The RELATE syntax:

```sql
RELATE (type::record('entities', $source))
    ->(type::record('relations', $id))
    ->(type::record('entities', $target))
SET relation_type = $rel_type, ...
```

This creates an edge record in `relations` with the source entity pointing `in` and the target entity pointing `out`. Because these are native SurrealDB record links (not string foreign keys), the `->` traversal operator works efficiently via index lookup rather than a full table scan:

```sql
-- One-hop traversal
(->relations->entities.*)

-- Three-hop traversal
(->relations->entities->relations->entities->relations->entities.*)
```

The `->` operator follows the `out` link of edge records. SurrealDB resolves each hop using the edge's built-in index, which is why multi-hop traversal doesn't require the application layer to issue N queries.

### 5. Composite Record IDs — The Structured KV

The `kv` table uses a deliberate naming scheme:

```rust
fn kv_id(agent_id: AgentId, key: &str) -> String {
    format!("{}:{}", agent_id.0, key)
}
// → kv:550e8400-e29b-41d4-a716-446655440000:self.user_name
```

In SurrealDB, record IDs can contain colons in complex forms. Here the format `{uuid}:{key}` is being embedded inside the `kv` table's record ID field. This gives O(1) point lookups by `SELECT kv:{agent_id}:{key}` without any secondary index hit. The composite ID effectively *is* the index.

### 6. Secondary Indexes — The Filter Indexes

```sql
DEFINE INDEX IF NOT EXISTS idx_memories_agent ON memories FIELDS agent_id;
DEFINE INDEX IF NOT EXISTS idx_memories_deleted ON memories FIELDS deleted;
```

These are B-tree indexes (SurrealDB's default for non-vector fields). They make `WHERE agent_id = $aid AND deleted = false` efficient. Without these, consolidation and recall would do full table scans of `memories`.

---

## What SurrealDB Offers That OpenFang Isn't Using

This is where the practical possibilities open up.

### LIVE SELECT — Real-Time Change Feeds

SurrealDB supports WebSocket-based live queries:

```sql
LIVE SELECT * FROM memories WHERE agent_id = $agent_id AND deleted = false;
```

A client subscribes to this query and receives differential events (`CREATE`, `UPDATE`, `DELETE`) in real-time whenever matching records change. This is built into the embedded mode too — `Surreal<Db>` can emit live events to Rust streams.

**What this could enable in OpenFang:** agents could reactively observe their own memory space. Instead of the current polling pattern in consolidation (run on a schedule, sweep all agents), a background Tokio task could hold a LIVE SELECT subscription on `memories WHERE scope = 'episodic'` and trigger L1 summarization immediately when episodic memory crosses a threshold — no scheduler needed. The TUI could render live memory updates without polling the API. Channel adapters could surface real-time "agent is thinking" state changes from the `agents` table.

The main constraint: the daemon holds the exclusive SurrealKV lock, so live queries only work on the same process as the write path — which is already true in OpenFang's embedded setup.

### DEFINE EVENT — Database-Native Hooks

```sql
DEFINE EVENT memory_created ON TABLE memories
    WHEN $event = "CREATE"
    THEN {
        -- Run something here
    };
```

Events fire inside the database whenever a record is created, updated, or deleted in a table. They run synchronously as part of the write transaction, so they're guaranteed to execute exactly once per write.

**The current OpenFang hook system** (`AfterMemoryStore`, `AfterMemoryRecall`) is implemented in Rust, outside the database, fired manually in `agent_loop.rs`. This means if the agent loop crashes between writing the memory and firing the hook, the hook silently doesn't run.

Moving to `DEFINE EVENT` would make hooks atomic with the write — the event fires or the write fails, never a partial state. It would also move observability triggers (emit metrics, update counters) closer to where the data changes.

The practical limitation: SurrealQL inside events can't call external services or async Rust code. Events are pure SurrealQL. So they work for in-database side-effects (update a counter, write a derived record) but not for triggering the NER pipeline or HTTP calls.

### DEFINE FUNCTION — Stored Query Logic

```sql
DEFINE FUNCTION fn::hybrid_score($vec_rank: float, $txt_rank: float) {
    RETURN ($vec_rank * 0.6) + ($txt_rank * 0.4);
};
```

Functions allow storing reusable SurrealQL logic in the database, parameterized and callable from queries. The hybrid scoring merge in `semantic.rs` is currently done in Rust after fetching two separate result sets. Moving the scoring into a database function would let you do a single query with an `ORDER BY fn::hybrid_score(...)` clause.

**Where this pays off:** when the HNSW and BM25 queries are merged in Rust, you're doing two round-trips and then sorting in process. A single SurrealQL query that joins both result sets and applies the hybrid score in-DB would cut two database calls to one. For high-frequency recall this matters.

### Transactions — The Missing Primitive

SurrealDB supports explicit transactions:

```sql
BEGIN;
-- ... multiple statements
COMMIT;
```

Or via the Rust SDK using the transaction API. **OpenFang has zero transactions.** The `task_claim` race condition is the most obvious casualty, but it's not the only one.

The `mark_summarized` method issues one `UPDATE` per memory ID in a loop:

```rust
for mid in memory_ids {
    self.db.query("UPDATE type::record('memories', $mid) ...").await?;
}
```

If this fails halfway through (e.g., a crash), some memories are marked as summarized and some aren't, leading to partial summarization state. A transaction wrapping the whole loop makes it atomic.

The practical issue with SurrealDB transactions in embedded mode: they work fine but there's no retry logic. A transaction conflict (another write touching the same records) causes an immediate error. For high-concurrency agent scenarios this needs handling.

### Native Datetime — The Timestamp Problem

Every timestamp in OpenFang is stored as an RFC3339 string:

```rust
let now = Utc::now().to_rfc3339();
// → "2026-04-26T05:00:00Z"
```

SurrealDB has a native `datetime` type. The practical consequence of using strings: time comparisons (`accessed_at < $cutoff`) work correctly only because RFC3339 strings sort lexicographically when they're in the same timezone. The moment you mix UTC with offset timestamps (e.g., `2026-04-26T05:00:00+00:00` vs `2026-04-26T05:00:00Z`), the string comparison breaks. The current code always generates UTC via `Utc::now().to_rfc3339()` so it's safe, but fragile.

Native `datetime` fields would enable:

```sql
-- Not currently possible with string timestamps
WHERE accessed_at < time::now() - 7d
WHERE created_at BETWEEN d'2026-01-01' AND d'2026-04-01'
```

And SurrealDB's time functions (`time::day()`, `time::floor()`) for grouping usage data by day without application-layer post-processing.

### Multi-Database Separation

OpenFang uses a single namespace `openfang` with a single database `agents`. Everything — memories, sessions, agents, usage, knowledge graph — lives in one database.

SurrealDB's namespace/database hierarchy enables proper separation:

```
openfang (namespace)
├── agents (database) — current, everything mixed
├── ops (database) — operator/admin tables
└── metrics (database) — usage/cost data isolated from agent data
```

Or more radically, per-agent databases:

```
openfang (namespace)
├── agent_550e8400 (database)
│   ├── memories (table)
│   └── kv (table)
└── agent_b0e6e2e1 (database)
    ├── memories (table)
    └── kv (table)
```

This would give true hard isolation between agents at the DB level rather than relying on `WHERE agent_id = $aid` predicates everywhere. The trade-off: you'd need one SurrealDB connection per database or a multiplexed connection pool, and cross-agent queries (consolidation's `all_agent_ids()`) become cross-database joins — messier in SurrealQL but possible.

### SurrealML — The Phase 3 Story

SurrealDB 3.x has initial SurrealML integration: ML models stored as DB objects that can be invoked from SurrealQL:

```sql
SELECT * FROM memories WHERE confidence > ml::predict('decay_model', {
    access_count: access_count,
    days_since_access: time::diff(accessed_at, time::now(), 'd')
});
```

This is explicitly Phase 3 in the OpenFang roadmap (`memories_merged: 0` in the consolidation report, marked as "SurrealML"). The idea: instead of the current threshold-based confidence decay (multiply by `0.95` if older than 7 days), a trained model embedded in the database predicts which memories to decay and which to keep based on access patterns, content type, and agent behaviour.

The practical constraint: SurrealML in v3 is still maturing. The API for embedding custom models is limited compared to external inference (Candle, Ollama). It's best suited for simple tabular models (gradient boosting, logistic regression) rather than transformer-based inference.

### DEFINE TABLE PERMISSIONS — DB-Level Capability Enforcement

Currently, capability enforcement in OpenFang is entirely in Rust:

```
tool_runner.rs → kernel.rs → is_system_principal() / check MemoryWrite(key) → memory.structured_set()
```

SurrealDB supports row-level permissions:

```sql
DEFINE TABLE kv SCHEMALESS
    PERMISSIONS
        FOR select WHERE agent_id = $auth.id
        FOR create, update WHERE agent_id = $auth.id
        FOR delete WHERE agent_id = $auth.id;
```

This would push the "agent can only read/write its own KV namespace" rule into the database itself, providing a second enforcement layer below the Rust kernel code. An authentication identity (`$auth`) would need to be established per-connection — which works in the network server mode of SurrealDB but is less natural in embedded mode where there's one shared connection for all agents.

---

## Where Things Should Work From an Application Perspective

Now the honest synthesis: given what OpenFang is and where it's going, what are the high-leverage changes in priority order?

### Priority 1 — Fix the Timestamp Type (Low Risk, High Correctness)

Migrating from RFC3339 strings to native SurrealDB `datetime` fields would:

- Enable real time arithmetic in SurrealQL (`accessed_at < time::now() - 7d` instead of computing a cutoff in Rust and passing a string).
- Make the consolidation queries more robust and readable.
- Enable time-based analytics on `usage` without application-layer grouping.

The migration path is a one-time backfill (SurrealDB can convert RFC3339 strings to datetime with `time::from::rfc3339()`). The SCHEMALESS nature of the tables means old string records and new datetime records can coexist during migration.

### Priority 2 — Transactions on Mutation Sequences (Medium Risk, High Correctness)

The two places that need this most:

**`task_claim`**: the SELECT + UPDATE should be a single atomic transaction. With a transaction and a `LIMIT 1` UPDATE returning the updated record, the claim becomes atomic.

**`mark_summarized`**: the per-memory loop should be wrapped in one transaction. Either all source memories get marked summarized or none do.

This doesn't require architectural changes — just wrapping existing SurrealQL in `BEGIN`/`COMMIT` blocks via the Rust SDK.

### Priority 3 — LIVE SELECT for the Consolidation Trigger (Medium Complexity, High Value)

The current consolidation scheduler runs on a fixed timer and sweeps all agents. It has no awareness of whether any agent actually generated new memories since the last run.

A LIVE SELECT subscription on the `memories` table could replace the fixed timer:

```sql
LIVE SELECT agent_id FROM memories
    WHERE scope = 'episodic' AND deleted = false;
```

When a new episodic memory is created, the live event fires, and the consolidation logic only needs to check the affected agent. This makes consolidation reactive rather than periodic and eliminates unnecessary work on idle agents.

The architectural point: this is particularly well-suited to the embedded model because the live subscription lives in the same process as the write path — zero latency between write and event delivery.

### Priority 4 — Separate the Agents Table from the Data Double-Serialization

The `agents` table storing `AgentEntry` as a JSON string inside a string field (`data: String`) is the biggest modeling debt in the schema. It makes the agents table opaque to SurrealQL — you can't filter by agent state, model, or capability without reading and deserializing every record in Rust.

The fix requires resolving why double-serialization was necessary (Rust enum variants in `AgentState`, `AgentMode`, etc. that the SurrealDB serializer couldn't handle). The `surreal_via_json!` macro already solves this problem for other types in the codebase — the same approach would work for `AgentEntry`. Once flattened, you could do:

```sql
SELECT name, state FROM agents WHERE state = 'Running';
SELECT * FROM agents WHERE manifest.model.provider = 'anthropic';
```

Without loading and deserializing every agent in Rust first.

### Priority 5 — Per-Agent Database Separation (High Complexity, Strong Isolation)

This is the long-range architectural question. The current single-database design with `WHERE agent_id = $aid` predicates everywhere means:

- Every query has to carry an agent filter — misformed queries could see cross-agent data.
- Capability enforcement is entirely in Rust; the database provides no isolation layer.
- Index selectivity degrades as agents × memories grows.

Moving to per-agent databases (or at minimum per-agent schemas) makes agent isolation structural rather than conventional. It does add complexity to the connection management layer and makes cross-agent operations (like the knowledge graph, which is currently global) require explicit cross-database queries.

The sweet spot is probably a **namespace-level split by data domain** rather than per-agent:

```
openfang/agents — agent registry, manifests
openfang/memory — memories, entities, relations, kv, sessions
openfang/ops — usage, audit, task_queue, paired_devices
```

This keeps the agent data collocated (for cross-agent recall, which is a feature) while separating operational/metering data from the hot agent memory path.

### Priority 6 — Graph-Memory Integration (Currently Disconnected)

The knowledge graph (`entities`, `relations`) and the semantic store (`memories`) are separate tables with no native SurrealDB connection. The only link is `metadata.entities` in a memory record — a JSON array of string IDs. This is a string foreign key, not a SurrealDB record link.

Making this a proper record link field:

```sql
DEFINE FIELD IF NOT EXISTS entities ON memories TYPE array<record<entities>>;
```

Would enable:

```sql
-- Find all memories that mention any entity connected to "Alice"
SELECT * FROM memories WHERE entities.*.name CONTAINS 'Alice';

-- Traverse from a memory through its entities to related entities
SELECT ->(entities->relations->entities) AS related FROM memories WHERE id = $mid;
```

This would unify the semantic and graph layers at the database level and make the graph-boosted recall (currently done by extracting entity IDs in Rust and then doing a second query) expressible as a single SurrealQL traversal.

---

## The Core Design Tension

The honest framing for all of this: SurrealDB's multi-model design gives OpenFang enormous surface area but also a significant footgun risk. Each model in SurrealDB has different consistency semantics, different index structures, and different performance characteristics. The current OpenFang design uses the models correctly in isolation but doesn't yet leverage the cross-model queries that make SurrealDB genuinely differentiated.

The `memories` table is the one place where the full multi-model surface is active: it's a document (arbitrary metadata), it has a vector index (HNSW), it has a full-text index (BM25), and it's linked to the graph (via `metadata.entities`). The knowledge graph is the other active piece. Everything else — sessions, KV, usage — could run on any document store without loss.

The architectural direction that would make the system genuinely SurrealDB-native rather than SurrealDB-as-a-document-store: making record links first-class everywhere (replacing string `agent_id` fields with actual record links to the `agents` table), collapsing the `metadata.entities` JSON array into a native `array<record<entities>>` field, and using LIVE SELECT to replace the polling patterns in consolidation and the TUI.

---

## Memory System Grey Areas (Reference)

These are the operationally significant inconsistencies and edge cases discovered during the substrate audit. They are documented here because each one intersects with the architectural priorities above.

### 1. HNSW Dimension Mismatch Between Docs and Code

The `semantic.rs` module-level doc says the HNSW index is **768d** to match `nomic-embed-text` (Ollama). The actual DDL in `db.rs` defines it as **384d** for `BAAI/bge-small-en-v1.5` (Candle). The DDL is what's deployed and the canonical doc is correct, but the `semantic.rs` header is wrong. If anyone reads that module doc and wires a 768d Ollama provider, their embeddings are silently excluded from HNSW results — they fall back to BM25 only with no error. There is no dimension validation at write time.

### 2. KV Store and Semantic Store Are Completely Separate Universes

When an agent calls `memory_store(key, value)`, it writes to the **`kv` table** via `structured_set`. When Layer 1 automatic pre-turn injection runs, it queries the **`memories` table** via HNSW + BM25. These are two different SurrealDB tables with no connection. A fact stored with `memory_store` is never surfaced by vector recall. It only comes back when the agent explicitly calls `memory_recall(key)` by exact name. The canonical doc states this, but agents that use `memory_store` expecting their facts to "come back in context" are broken.

### 3. `task_claim` Has a Race Condition and Returns the Wrong ID

**Race:** `task_claim` is a SELECT then a separate UPDATE — not atomic. Two agents claiming simultaneously can both see the same `pending` task, both claim it, and both receive it as `in_progress`.

**ID bug:** `task_claim` returns the JSON field `"id": task.title` (the title string). But `task_complete(task_id, result)` uses `type::record('task_queue', $tid)` where `$tid` is that value. The actual SurrealDB record key is a UUID. Passing the title string to `task_complete` silently fails to find the record — the update fires and touches zero rows with no error.

### 4. Soft-Deleted Memories Stay in the HNSW Index

When `forget()` or `prune()` marks a memory `deleted = true`, it stays as a record with an embedding. The HNSW index has no predicate — it indexes every record with an embedding. Deleted memories are retrieved by the KNN query and then discarded in Rust after the fact. The 3× oversample in `recall_vector` may not be enough in a heavily-pruned collection. Pruned memories continue to consume HNSW graph memory and slow graph traversal until the DB is reset or `purge_deleted()` is called. `purge_deleted` is implemented but there is no scheduled path that calls it.

### 5. Access Count Updates Are Best-Effort and Non-Atomic

After every recall, `accessed_at` and `access_count` are updated in a sequential loop that discards errors. If these fail, `accessed_at` stays stale. Since `decay_confidence` uses `accessed_at < cutoff` to decide which memories to decay, a memory that's being actively recalled but whose `accessed_at` hasn't updated will be incorrectly decayed on the next consolidation run.

### 6. `ConsolidationReport::memories_merged` Is Always 0

The `Memory::consolidate()` implementation hardcodes `memories_merged: 0` with a comment that merge-by-similarity is "Phase 3 (SurrealML)". The field exists in the public type returned from the API, so any tooling or dashboards reading that field are silently getting incorrect data.

### 7. `export()` and `import()` Are Unimplemented Stubs

`export()` returns `Ok(Vec::new())` — an empty byte vec with no error. A caller that writes that to disk has a 0-byte file with no indication anything is wrong. `import()` at least returns an error string, but it's still `Ok(...)` at the `Result` level.

### 8. KV `version` Field Is Decorative

`StructuredStore::set` always writes `version: 1`. It never reads, increments, or compares the existing version. The field exists in `KvRecord` but is not versioning anything — it's a misleading field name.

### 9. `ConsolidationReport` Conflates Decayed and Pruned

`Memory::consolidate()` adds `decayed` (confidence reduced, still alive) and `pruned` (soft-deleted) into a single `memories_decayed` counter. The distinction matters: a memory that got nudged from `0.11` to `0.10` and then was pruned is counted twice into the same field.

### 10. The System Principal UUID Is the Shared Memory UUID

`SYSTEM_AGENT_ID = "00000000-0000-0000-0000-000000000001"` (used to bypass capability checks) is the same UUID as `shared_memory_agent_id()` (used as the storage key for the shared KV namespace). Any code path that calls a memory tool with `agent_id = shared_memory_agent_id()` will bypass capability enforcement entirely. This is correct for internal infra, but if an agent were ever assigned this UUID through a registration bug, it would have unchecked write access to all namespaces.

### 11. BOOTSTRAP.md Suppression Scope Is Agent-Local

Private `self.user_name` is agent-local KV and is not a global bootstrap gate. Multi-agent setups that want bootstrap suppression to cross agent boundaries need either a `shared.user_name` (with `memory_write = ["shared.*"]` capability) or a pre-populated `USER.md` file in the workspace.

### 12. `update_metadata` Replaces All Metadata — No Merge

If `update_metadata` is called after NER has already populated `metadata.entities`, the entity links are wiped. The backfill pipeline calls `update_metadata` in some paths. If NER and classification run concurrently for the same memory record, one write will overwrite the other's metadata.

### 13. `fetch_episodic_batch` Has an Ambiguous NONE Check

The query for finding unsummarized episodic memories uses:

```sql
AND (metadata.summarized_into = NONE OR !metadata.summarized_into)
```

`= NONE` checks for SurrealDB's `NONE` (missing field). `!metadata.summarized_into` catches `null` or falsy values. But if the field was ever set and then cleared to `""` (empty string), `!""` is `true` in SurrealDB — that memory would be re-summarized.

---

## Session Compaction Defaults (Operational Note)

A separate but related operational concern: `CompactionConfig::default()` in `crates/openfang-runtime/src/compactor.rs` is hardcoded with `threshold: 30` and `keep_recent: 10`. These values cannot be overridden via `config.toml` or `agent.toml` — the kernel always calls `CompactionConfig::default()`. Conversational agents hit the 30-message threshold quickly and have their session truncated to the last 10 verbatim messages, with everything else replaced by a single LLM-generated summary paragraph. The JSONL mirrors at `~/.openfang/sessions/<session_uuid>.jsonl` retain the full verbatim record and are not affected by compaction.

Adding compaction config to `KernelConfig` (and threading it through the two call sites in `kernel.rs`) is a small change with significant operational impact for any deployment that does long conversations.

---

## Summary

| Layer | Currently Used | Practical Surface Available |
|---|---|---|
| Document store | All 9 tables | Could collapse double-serialized `agents.data` |
| HNSW vector index | `memories.embedding` (384d) | Could add second index for hypothetical embeddings |
| BM25 full-text | `memories.content` | `HIGHLIGHTS` available for snippets, unused |
| Graph (RELATE) | `entities` ↔ `relations` | Memory records could be linked into the graph |
| Composite record IDs | `kv:{agent_id}:{key}` | Pattern could extend to `usage`, `task_queue` |
| Secondary indexes | `agent_id`, `deleted`, `created_at` | `metadata.scope`, `metadata.summarized_into` likely beneficial |
| LIVE SELECT | None | Reactive consolidation, real-time TUI |
| DEFINE EVENT | None | Atomic write hooks |
| DEFINE FUNCTION | None | In-DB hybrid scoring |
| Transactions | None | `task_claim`, `mark_summarized` |
| Native datetime | None (RFC3339 strings) | Time arithmetic, analytics |
| Multi-database | None (single `agents` DB) | Domain or per-agent isolation |
| SurrealML | None | Future memory merge / decay model |
| Table permissions | None | DB-level capability enforcement |
