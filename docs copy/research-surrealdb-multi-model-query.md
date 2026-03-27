# SurrealDB Multi-Model Architecture & Query Engine

> Research report for pattern extraction. Sources: SurrealDB docs (surrealdb.com/docs), GitHub repo (surrealdb/surrealdb v3.1.0-alpha), blog posts (3.0 release, benchmarks). BSL-1.1 license for core code.

## 1. Multi-Model Data Representation

### The Unifying Principle: Everything Is a Record in a Table on a KV Store

SurrealDB is, at its core, a document database. Every record is stored as a serialized value in the underlying key-value storage engine. The multi-model capability is not achieved by having separate storage paths for different models — instead, **all models share a single record abstraction** and differ only in how SurrealQL interprets and queries them.

**Record ID structure:** Every record has an ID of the form `table_name:record_key`. The table name prefix is the record link — a direct pointer into the KV store. This is why record links are "extremely efficient": resolving `user:tobie` is a single KV lookup, not a table scan.

**Internal storage layout (inferred from key encoding blog post):**

Keys follow a structured encoding scheme using the `storekey` crate. The format uses prefix bytes to separate namespaces:
- `/*` — namespace catalog entries
- `*` — database catalog entries
- `+` — index entries
- `!` — data entries

In v3.0, catalog entries moved from name-based to ID-based storage. Previously a key like `/\*namespace\0*database\0*order_history\0+order_history__city_time__idx\0!bd...` was 80 bytes; now with compact fixed-size namespace/database IDs it's 42 bytes.

### Document Model

Records are JSON-like objects stored as serialized values. Each record is a table row with arbitrary fields (schemaless) or typed fields (schemafull). Nested objects, arrays, and all SurrealDB types are stored inline.

### Graph Model — Edges Are Tables

Graph edges are **not** stored in a separate graph-specific data structure. They are **regular SurrealDB table records** with two mandatory fields:

- `in` — the source record ID (the subject)
- `out` — the target record ID (the object)

The `RELATE` statement (`RELATE user:tobie->wrote->article:surreal SET ...`) creates a record in the edge table (e.g., `wrote`) with `in = user:tobie` and `out = article:surrealdb`. The edge table is a fully normal table — it can have its own fields, indexes, and schema definitions.

A table can be declared `TYPE RELATION` with optional `IN` and `OUT` type constraints:
```surql
DEFINE TABLE likes TYPE RELATION IN person OUT blog_post | book;
```

This is purely a schema-level constraint. At the storage level, it's still a KV record with `in`/`out` fields.

### Record Links vs Graph Edges

Two mechanisms for relating records:

| Mechanism | Storage | Bidirectional? | Metadata? |
|-----------|---------|-----------------|-----------|
| Record link | Field value holding a record ID | Only with `REFERENCE` clause (v3.0) | No |
| Graph edge | Separate table record with `in`/`out` | Yes (via `<->` operator) | Yes (arbitrary fields) |

**Record links** are just a field containing a record ID. They're resolved by a single KV read. Since v3.0, they can be made bidirectional with `DEFINE FIELD comments ON user TYPE option<array<record<comment>>> REFERENCE`.

**Graph edges** are a separate table. Traversal requires querying the edge table (e.g., `SELECT * FROM wrote WHERE in = $parent.id`), which can use indexes on `in`/`out`. The `->` and `<-` arrow operators in SurrealQL compile to these lookups.

### Relational Model

The relational model is "free" — it emerges from having tables with typed fields, indexes, and SQL-like querying. There are no separate relational storage structures. Schema enforcement (`SCHEMAFULL` tables) validates field types on write. Indexes (unique, composite, full-text, HNSW) provide the performance characteristics expected from a relational DB.

### KV Model

Direct KV access is available through record IDs. Since every record is stored at a known key (`table_name:record_key`), you can do direct point reads without any query planning.

## 2. Query Engine Design

### Architecture (v3.0 rewrite)

The v3.0 release introduced a fundamentally new execution engine. The pipeline is:

```
SurrealQL text → Parser → AST → LogicalPlan → ExecutionPlan → Results
```

Previously, the engine evaluated queries in a more ad-hoc manner. The new model is **fully streaming internally** (end-to-end streaming planned for a future minor release).

### Pipeline Stages (from architecture docs)

1. **Parser** — Runs SurrealQL through a parser, returning a parsed query definition (AST).
2. **Executor** — Breaks up each statement within the query, managing transactions across statements.
3. **Iterator** — Determines which data to fetch from the KV store. Handles **index query planning**, grouping, ordering, and limiting.
4. **Document processor** — Processes permissions, determines which data is merged/altered, and stores on disk.

### Query Planner Optimizations (v3.0)

The v3.0 planner is smarter than v2.x:

- **ID-based primary key lookups**: `SELECT * FROM table WHERE id = record:42` previously triggered a full table scan. Now it resolves in sub-millisecond time (4000x+ improvement). This is critical because `WHERE id =` is one of the most common SurrealQL patterns.
- **Index selection**: The iterator stage handles index query planning. When a WHERE clause matches an available index, the planner routes to the index instead of a full scan.
- **Streaming execution**: The new engine streams results internally rather than materializing full intermediate result sets.

### Graph Traversal Execution

Graph queries using `->` and `<-` operators compile to subqueries against edge tables:

```
SELECT ->wrote->post.* FROM users:alice
```

This becomes (conceptually):
1. Look up `users:alice` (single KV read)
2. Query the `wrote` table WHERE `in = users:alice` (index scan on `in`)
3. For each edge, look up `out` record in the `post` table (batch KV reads)

**Automatic flattening**: Multi-hop traversals like `->knows->(? AS f1)->knows->(? AS f2)` maintain the original array structure rather than creating deeply nested arrays. This is described as "maintaining the original structure" rather than post-query flattening.

**Recursive queries**: `@.{N}->edge->table` syntax repeats a traversal N times, building nested output objects.

### Performance Characteristics (from v3.0 benchmarks)

| Workload | v3.0 improvement |
|----------|----------------|
| Graph depth-1 traversal | 8-22x faster |
| Graph depth-2 traversal | 8-22x faster |
| Graph depth-3 traversal | 4-8x faster |
| `SELECT ... WHERE id =` (indexed) | 4000x+ faster |
| `ORDER BY` queries | 3-4x faster |
| HNSW vector search | Up to 8x faster |
| `GROUP BY` with multiple aggregations | Up to 55% faster |
| `LIMIT` on large scans | 3-6x faster |

## 3. Codebase Organization

### Workspace Structure

```
surrealdb/              # Root crate — binary entry point, feature flags
├── surrealdb/          # SDK crate — public API for embedded use
├── surrealdb/core/     # Core engine — query execution, storage, catalog
├── surrealdb/server/   # Server — HTTP, WebSocket, RPC, gRPC endpoints
├── surrealdb/types/    # Public types and derive macros
│   └── derive/         # Procedural macros for type generation
├── surrealml/core/     # ML inference (ONNX Runtime integration)
├── surrealism/         # WASM plugin system
│   ├── demo/
│   ├── macros/
│   ├── runtime/
│   └── types/
├── language-tests/     # SurrealQL test suite (.surql files)
└── tests/              # Integration tests (CLI, HTTP, WebSocket, GraphQL)
```

### Core Crate Modules (`surrealdb/core/src/`)

| Module | Responsibility |
|--------|---------------|
| `sql/` | SurrealQL parser — produces AST from query text |
| `syn/` | Syntax definitions, lexer, token types |
| `exe/` | Query executor — orchestrates statement execution |
| `exec/` | Execution plans and iterators |
| `expr/` | Expression evaluation (the "values" layer, split from `val/` in v3.0) |
| `val/` | Value types — the data model (records, arrays, objects, geometry, etc.) |
| `doc/` | Document processor — permissions, merge logic, storage |
| `idx/` | Index definitions and implementations (standard, composite, unique, full-text, HNSW, range) |
| `key/` | Key encoding — structured key format for the KV store |
| `kvs/` | KV store abstraction — trait implementations for RocksDB, SurrealKV, TiKV, memory, IndexedDB |
| `cat/` | Catalog — namespace, database, table, index, user definitions |
| `ctx/` | Context — per-query context (transaction, database, permissions) |
| `dbs/` | Database — database-level operations |
| `api/` | Custom API endpoints (`DEFINE API`) |
| `buc/` | Background tasks |
| `cnf/` | Configuration |
| `cf/` | Configuration fields |
| `env/` | Environment |
| `err/` | Error types |
| `fnc/` | Function registry — built-in SurrealQL functions |
| `gql/` | GraphQL integration |
| `iam/` | Authentication and authorization |
| `mac/` | Macros |
| `mem/` | Memory management |
| `obs/` | Observability |
| `rpc/` | RPC protocol |
| `str/` | String utilities |
| `surrealism/` | Surrealism WASM plugin integration |
| `sys/` | System info |
| `fmt/` | Formatting |

### Key Abstractions

**Storage trait** (`kvs/`): The KV store is abstracted behind a trait that supports transactions, key-range scans, and individual key reads/writes. Implementations exist for:
- Memory (lock-free, for testing/embedded)
- RocksDB (LSM tree, single-node persistent)
- SurrealKV (custom LSM tree, single-node persistent, time-travel queries)
- TiKV (distributed, Raft-based)
- IndexedDB (browser/WASM)

**Key encoding** (`key/`): Uses the `storekey` crate for structured, ordered key encoding. Keys are prefixed with type markers (`/*`, `*`, `+`, `!`) so that catalog entries, index entries, and data entries sort correctly and can be scanned efficiently.

**Value/Expression split** (`val/` vs `expr/`): In v3.0, values (what data IS) were separated from expressions (how data is DERIVED). Previously these were entangled, causing redundant computation and edge-case bugs. Now the engine evaluates expressions only when needed.

**Document wrapper** (`doc/`): v3.0 introduced a proper document wrapper type that separates record content from record metadata. Previously metadata was stored as hidden fields within the record value.

### External Dependencies of Note

| Crate | Purpose |
|-------|---------|
| `surrealkv` | Custom LSM-tree storage engine (v0.21.0) |
| `surrealmx` | In-memory storage with time travel |
| `rocksdb` (forked) | RocksDB storage backend |
| `tikv` (forked) | TiKV distributed storage backend |
| `revision` | Versioned data structures (used for document change tracking) |
| `storekey` | Structured key encoding |
| `rquickjs` | Embedded JavaScript runtime (scripting) |
| `async-graphql` | GraphQL engine |
| `fst` | Finite state transducer (full-text index) |
| `geo` / `geo-types` | Geospatial types |
| `ort` | ONNX Runtime (ML inference) |

## 4. Transaction Integration

### DB-Level Transaction Model

SurrealDB supports **multi-row, multi-table ACID transactions**. The transaction model sits on top of the underlying KV store's transaction support.

**How it works:**

1. The **executor** receives a SurrealQL query (which may contain multiple statements).
2. For a `BEGIN`/`COMMIT` block (or implicit single-statement transaction), the executor acquires a transaction from the KV store.
3. All statements within the transaction share the same KV transaction, seeing each other's writes.
4. On `COMMIT`, the KV transaction is committed atomically. On `CANCEL`, it's rolled back.

**Multi-table transactions**: Since all tables store their data in the same KV store (just with different key prefixes), a single KV transaction naturally spans multiple tables. There are no table-level or row-level locks — the KV store's concurrency control (optimistic or pessimistic, depending on the backend) handles conflicts.

**Distributed transactions** (TiKV): When using TiKV, multi-table transactions use TiKV's distributed transaction protocol (inspired by Google Percolator). The docs note that "SurrealDB makes use of special techniques when handling multi-table transactions, and document record IDs — with no use of table or row locks."

### Client-Side Transactions (v3.0)

v3.0 introduced client-side transactions: begin → add operations across multiple requests → commit or cancel. The transaction state is stored server-side, and the logic for commit/cancel lives in the client SDK. This enables multi-step workflows that span time or human intervention while maintaining ACID guarantees.

### Storage Engine Transaction Models

| Backend | Concurrency | Transaction Model |
|---------|-------------|-------------------|
| Memory | Concurrent readers and writers | Lock-free in-memory |
| RocksDB | Concurrent readers, single writer | Optimistic with snapshots |
| SurrealKV | Concurrent readers and writers | ACID with versioned keys |
| TiKV | Concurrent readers and writers | Distributed ACID (Raft + Percolator) |
| IndexedDB | Single writer (browser) | Transaction API |

### Synced Writes Default (v3.0)

v3.0 changed the default to synced writes — the engine confirms a write only after it's durably committed to disk. Previously it relied on OS flush, which could lose data on crash.

## 5. Key Architectural Patterns

### Pattern 1: Single KV Abstraction, Multiple Models

The core insight is that **document, graph, relational, and KV models are all views over the same underlying key-value storage**. There is no separate graph engine, no separate relational engine. The `->` operator compiles to a subquery against an edge table. Schema enforcement is validation on write. This eliminates the N+1 query problem for record links (single KV lookup) and avoids the complexity of synchronizing multiple storage engines.

**Relevance to Mind**: This is the opposite of Mind's approach. Mind uses `redb` + `usearch` + `petgraph` as separate stores. SurrealDB's approach trades query flexibility for implementation simplicity. Mind's approach trades implementation complexity for the ability to use purpose-built data structures for each subsystem.

### Pattern 2: Compute-Storage Separation

The query layer (compute) is completely separated from the storage layer. The storage layer is a trait that can be swapped: memory, RocksDB, SurrealKV, TiKV, IndexedDB. This enables the same binary to run embedded, single-node, or distributed. The docs explicitly describe this as a "layered approach, with compute separated from storage."

**Relevance to Mind**: Mind is a single binary with embedded storage (`redb`). The compute-storage separation pattern is relevant if Mind ever needs to support multiple storage backends or distributed deployment.

### Pattern 3: Value-Expression Separation (v3.0)

Splitting "what data is" from "how data is derived" is a significant architectural choice. It enables:
- The query planner to optimize expression evaluation
- Computed fields that are evaluated at query time, not stored per-record
- A cleaner path to future query optimizations

**Relevance to Mind**: Mind's assertion/inquiry tension has a parallel. The "value" layer is like assertions (committed beliefs), and the "expression" layer is like inquiries (active evaluation). The explicit separation makes it easier to reason about which computations are cached vs live.

### Pattern 4: ID-Based Catalog Storage

Moving from name-based to ID-based storage for catalog entries (namespaces, databases, indexes) reduces key size and enables future renaming. The key encoding uses compact fixed-size IDs where possible.

**Relevance to Mind**: Mind's schema store (if using `redb`) could benefit from ID-based rather than name-based keys for internal metadata.

### Pattern 5: Document Wrapper (Content vs Metadata Separation)

v3.0 introduced a proper document wrapper that separates record content from metadata. Previously metadata was stored as hidden fields, causing correctness issues with views and statistics. This is a common pattern in mature databases (e.g., PostgreSQL's system columns).

**Relevance to Mind**: When storing trace entries, separating the trace content from metadata (timestamp, source, confidence) at the storage level avoids the "hidden fields" problem.

### Pattern 6: Streaming Execution Pipeline

The v3.0 engine is fully streaming internally (AST → LogicalPlan → ExecutionPlan). This means intermediate results don't need to be fully materialized. For large result sets, this significantly reduces memory pressure.

**Relevance to Mind**: The orchestrator's retrieval pipeline should stream results rather than materializing full result sets, especially for graph traversals across the similarity index.

### Pattern 7: Edge Tables as First-Class Citizens

Graph edges are regular tables with `in`/`out` fields, not a special graph data structure. This means:
- Edges can have their own schema, indexes, and permissions
- The same query engine handles both document and graph queries
- No separate graph engine to maintain or synchronize

**Trade-off**: Graph traversals compile to subqueries, which may be slower than a native graph engine for deep traversals. However, the v3.0 benchmarks show 8-22x improvements, suggesting the query planner is effective at optimizing these patterns.

**Relevance to Mind**: If Mind needs graph-like relationships (e.g., between trace entries), using regular records with reference fields (like SurrealDB's `REFERENCE` clause) rather than a separate graph structure is simpler and avoids synchronization overhead.
