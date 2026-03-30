# SurrealDB + OpenFang Integration Reference

Compiled from SurrealDB docs, blog posts, docs.rs, GitHub, and OpenFang documentation.
SurrealDB's main docs site (surrealdb.com/docs) blocks automated access, so this reference
is assembled from publicly accessible search results, blog posts, and API documentation.

---

## 1. Embedded Rust SDK

### Connection (SurrealKV — what OpenFang uses)

```rust
use surrealdb::engine::local::SurrealKv;
use surrealdb::Surreal;

let db = Surreal::new::<SurrealKv>("path/to/data.db").await?;
db.use_ns("openfang").use_db("agents").await?;
```

Feature flag in Cargo.toml: `surrealdb = { version = "2.5", features = ["kv-surrealkv"] }`

### Alternative: In-Memory (for tests)

```rust
use surrealdb::engine::local::Mem;
let db = Surreal::new::<Mem>(()).await?;
```

Feature flag: `kv-mem`

### Dynamic Engine Selection (Surreal\<Any\>)

```rust
use surrealdb::engine::any;
let endpoint = std::env::var("SURREALDB_ENDPOINT").unwrap_or("memory".to_owned());
let db = any::connect(endpoint).await?;
```

Infers engine from URL scheme at runtime. Develop with `memory`, deploy with `surrealkv://path`.

### Performance Profile (recommended for release builds)

```toml
[profile.release]
lto = true
strip = true
opt-level = 3
panic = 'abort'
codegen-units = 1
```

Tokio configuration for embedded use:

```rust
tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .thread_stack_size(10 * 1024 * 1024) // 10MiB
    .build()
    .unwrap()
    .block_on(async { /* app */ })
```

### SDK Version Compatibility

Rust SDK works with SurrealDB v2.0.0 through v3.0.4. Requires Rust 1.89+.

---

## 2. SurrealKV Storage Engine

SurrealKV is SurrealDB's pure-Rust embedded storage, replacing RocksDB dependency.

- **Architecture:** LSM (Log-Structured Merge) tree
- **ACID compliant:** full atomicity, consistency, isolation, durability
- **MVCC:** snapshot isolation with non-blocking concurrent reads/writes
- **Time-travel queries:** built-in versioning with point-in-time reads
- **Checkpoint/restore:** consistent snapshots for backup and recovery
- **Value log (Wisckey):** large values stored separately with GC
- **Handles larger-than-memory datasets**

---

## 3. Data Models Available

### Document (currently used by OpenFang)

Standard schemaless or schemafull document storage. JSON-like records with nested objects/arrays.

```sql
CREATE agent:researcher SET
    name = "Researcher",
    config = { model: "claude-sonnet", temperature: 0.7 },
    created_at = time::now();
```

### Graph (currently used — single hop only)

Records link via `RELATE`. Traversal with `->` (outbound) and `<-` (inbound).

```sql
-- Create a graph edge
RELATE entity:openai->mentions->entity:gpt4 SET
    context = "product announcement",
    confidence = 0.95,
    discovered_at = time::now();

-- Traverse outbound 1 hop
SELECT ->mentions->entity FROM entity:openai;

-- Traverse 2 hops
SELECT ->mentions->entity->mentions->entity FROM entity:openai;

-- Traverse with filtering
SELECT ->mentions->(entity WHERE confidence > 0.8) FROM entity:openai;

-- Bidirectional
SELECT <-mentions<-entity AS mentioned_by FROM entity:gpt4;
```

**DEFINE TABLE ... RELATION** enforces that a table can only hold graph edges:

```sql
DEFINE TABLE mentions TYPE RELATION;
```

### Vector (HNSW — not yet indexed in OpenFang)

Native HNSW indexing with KNN operator.

```sql
-- Define table and field
DEFINE TABLE memories SCHEMAFULL;
DEFINE FIELD content ON memories TYPE string;
DEFINE FIELD embedding ON memories TYPE array<float>;
DEFINE FIELD agent_id ON memories TYPE string;
DEFINE FIELD created_at ON memories TYPE datetime DEFAULT time::now();

-- Define HNSW index (dimension MUST match embedding model)
DEFINE INDEX hnsw_embedding ON memories FIELDS embedding
    HNSW DIMENSION 384 DIST COSINE;

-- KNN query (brute force — uses algorithm name)
SELECT content, vector::distance::knn() AS distance
FROM memories
WHERE embedding <|5,COSINE|> $query_embedding
ORDER BY distance;

-- KNN query (HNSW indexed — uses ef parameter)
SELECT content, vector::distance::knn() AS distance
FROM memories
WHERE embedding <|5,40|> $query_embedding
ORDER BY distance;
```

**Known bug (Feb 2026):** HNSW via remote Rust SDK may return all records instead of K.
Embedded SurrealKV is not affected.

**Quantization support:** `TYPE I32`, `TYPE I16`, `TYPE F32`, `TYPE F64` for balancing
precision vs memory.

### Full-Text Search / BM25 (not yet used by OpenFang)

```sql
-- Define analyzer
DEFINE ANALYZER memory_analyzer
    TOKENIZERS blank, class, camel
    FILTERS lowercase, snowball(english);

-- Define search index with BM25
DEFINE INDEX memory_content ON memories FIELDS content
    SEARCH ANALYZER memory_analyzer BM25(1.2, 0.75) HIGHLIGHTS;

-- Full-text query
SELECT content,
    search::score(1) AS score,
    search::highlight('<b>', '</b>', 1) AS highlighted
FROM memories
WHERE content @1@ 'rust async migration'
ORDER BY score DESC
LIMIT 10;
```

### Hybrid Search (vector + full-text combined)

```sql
LET $query = "How does memory consolidation work?";
LET $embedding = $query_embedding;  -- pre-computed

SELECT id, content,
    vector::similarity::cosine(embedding, $embedding) AS vs,
    search::score(1) AS ts,
    (vs * 0.6) + (ts * 0.4) AS score
FROM memories
WHERE embedding <|4,COSINE|> $embedding
   OR content @1@ $query
ORDER BY score DESC
LIMIT 5;
```

### Time Series (not yet used by OpenFang — relevant for usage/metering)

Optimized for time-stamped data with aggregated table views.

```sql
-- Usage events with temporal indexing
DEFINE TABLE usage_events SCHEMAFULL;
DEFINE FIELD agent_id ON usage_events TYPE string;
DEFINE FIELD model ON usage_events TYPE string;
DEFINE FIELD tokens_in ON usage_events TYPE int;
DEFINE FIELD tokens_out ON usage_events TYPE int;
DEFINE FIELD cost ON usage_events TYPE float;
DEFINE FIELD timestamp ON usage_events TYPE datetime DEFAULT time::now();

-- Aggregated view for daily summaries
DEFINE TABLE usage_daily AS
    SELECT
        agent_id,
        model,
        math::sum(tokens_in) AS total_in,
        math::sum(tokens_out) AS total_out,
        math::sum(cost) AS total_cost,
        count() AS request_count,
        time::floor(timestamp, 1d) AS day
    FROM usage_events
    GROUP BY agent_id, model, day;
```

### Geospatial

Points, lines, polygons, MultiPoint, etc. Not currently relevant for OpenFang.

---

## 4. Schema DDL (what OpenFang's db::init() should contain)

### Current state: zero DDL — everything is schemaless auto-created

### Recommended DDL for db::init():

```sql
-- Namespace and database
USE NS openfang DB agents;

-- Analyzer for full-text search
DEFINE ANALYZER IF NOT EXISTS memory_analyzer
    TOKENIZERS blank, class, camel
    FILTERS lowercase, snowball(english);

-- Sessions table
DEFINE TABLE IF NOT EXISTS sessions SCHEMAFULL;
DEFINE FIELD agent_id ON sessions TYPE string;
DEFINE FIELD messages ON sessions TYPE array;
DEFINE FIELD created_at ON sessions TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at ON sessions TYPE datetime DEFAULT time::now();

-- Memories table (semantic store)
DEFINE TABLE IF NOT EXISTS memories SCHEMAFULL;
DEFINE FIELD agent_id ON memories TYPE string;
DEFINE FIELD content ON memories TYPE string;
DEFINE FIELD embedding ON memories TYPE option<array<float>>;
DEFINE FIELD confidence ON memories TYPE float DEFAULT 1.0;
DEFINE FIELD created_at ON memories TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at ON memories TYPE datetime DEFAULT time::now();

-- HNSW vector index (384 = MiniLM dimension)
DEFINE INDEX IF NOT EXISTS hnsw_embedding ON memories
    FIELDS embedding HNSW DIMENSION 384 DIST COSINE;

-- BM25 full-text index
DEFINE INDEX IF NOT EXISTS ft_content ON memories
    FIELDS content SEARCH ANALYZER memory_analyzer BM25(1.2, 0.75) HIGHLIGHTS;

-- Entities table (knowledge graph nodes)
DEFINE TABLE IF NOT EXISTS entities SCHEMAFULL;
DEFINE FIELD name ON entities TYPE string;
DEFINE FIELD entity_type ON entities TYPE string;
DEFINE FIELD metadata ON entities TYPE object DEFAULT {};

-- Relations table (knowledge graph edges)
DEFINE TABLE IF NOT EXISTS relations TYPE RELATION;
DEFINE FIELD relationship ON relations TYPE string;
DEFINE FIELD confidence ON relations TYPE float DEFAULT 1.0;
DEFINE FIELD context ON relations TYPE option<string>;

-- Structured store (agent state)
DEFINE TABLE IF NOT EXISTS agents SCHEMAFULL;
DEFINE FIELD name ON agents TYPE string;
DEFINE FIELD data ON agents TYPE object;

-- Usage events
DEFINE TABLE IF NOT EXISTS usage_events SCHEMAFULL;
DEFINE FIELD agent_id ON usage_events TYPE string;
DEFINE FIELD model ON usage_events TYPE string;
DEFINE FIELD tokens_in ON usage_events TYPE int;
DEFINE FIELD tokens_out ON usage_events TYPE int;
DEFINE FIELD cost ON usage_events TYPE float;
DEFINE FIELD timestamp ON usage_events TYPE datetime DEFAULT time::now();

-- Consolidation records
DEFINE TABLE IF NOT EXISTS consolidation SCHEMAFULL;
DEFINE FIELD agent_id ON consolidation TYPE string;
DEFINE FIELD decayed ON consolidation TYPE int DEFAULT 0;
DEFINE FIELD pruned ON consolidation TYPE int DEFAULT 0;
DEFINE FIELD merged ON consolidation TYPE int DEFAULT 0;
DEFINE FIELD ran_at ON consolidation TYPE datetime DEFAULT time::now();
```

---

## 5. Live Queries (Rust SDK)

Subscribes to record changes in real time. Works with embedded engines (SurrealKV, Mem).

```rust
use surrealdb::method::Stream;

// Subscribe to all changes on a table
let mut stream = db.select("memories").live().await?;

// Subscribe to a single record
let mut stream = db.select(("memories", "abc123")).live().await?;

// Process notifications
while let Some(notification) = stream.next().await {
    match notification?.action {
        Action::Create => { /* new record */ },
        Action::Update => { /* modified record */ },
        Action::Delete => { /* removed record */ },
    }
}
```

Relevant for: pushing session updates and Hand status to the dashboard via SSE
without polling.

---

## 6. SurrealML (ONNX inference inside the database)

### Architecture

- Models trained in Python (PyTorch, TensorFlow, Sklearn)
- Exported to `.surml` format (ONNX + metadata)
- Imported into SurrealDB instance
- Inference runs in Rust-native ONNX runtime (embedded in SurrealDB binary)
- CPU and GPU computation at query time

### Import model

```bash
surreal ml import --ns openfang --db agents embedding_model.surml
```

### Query-time inference

```sql
-- Raw compute
RETURN ml::embedding::minilm<0.1.0>([1.0, 2.0, 3.0]);

-- Buffered compute with named inputs
SELECT *,
    ml::embedding::minilm<0.1.0>({ text: content }) AS embedding
FROM memories
WHERE embedding IS NONE;
```

### Rust SDK — loading and running locally

```toml
[dependencies]
surrealml-core = "0.1"  # check latest version
```

```rust
use surrealml_core::storage::surml_file::SurMlFile;
use surrealml_core::execution::compute::ModelComputation;
use ndarray::ArrayD;
use std::collections::HashMap;

// Load model
let mut file = SurMlFile::from_file("embedding.surml")?;

// Buffered compute (uses metadata for normalization)
let compute = ModelComputation { surml_file: &mut file };
let mut inputs = HashMap::new();
inputs.insert("text".into(), 1.0); // simplified
let output = compute.buffered_compute(&mut inputs)?;

// Raw compute (bypass metadata)
let data = ndarray::arr1(&[1.0, 2.0, 3.0]).into_dyn();
let output = compute.raw_compute(data, None)?;
```

### Key consideration for OpenFang

SurrealML's ONNX runtime is embedded in the SurrealDB binary. If OpenFang embeds
SurrealDB via SurrealKV, the ONNX runtime should be available for in-query inference.
**This needs verification** — confirm the `ml` feature flag is available for
embedded/local engines, not just the server binary.

---

## 7. OpenFang Architecture (from README + docs)

### Crate structure (14 crates)

| Crate | Responsibility |
|-------|---------------|
| openfang-kernel | Orchestration, workflows, metering, RBAC, scheduler, budget |
| openfang-runtime | Agent loop, 3 LLM drivers, 53 tools, WASM sandbox, MCP, A2A |
| openfang-api | 140+ REST/WS/SSE endpoints, OpenAI-compatible API, dashboard |
| openfang-channels | 40 messaging adapters with rate limiting, DM/group policies |
| openfang-memory | SurrealDB persistence, vector embeddings, sessions, compaction |
| openfang-types | Core types, taint tracking, Ed25519 signing, model catalog |
| openfang-skills | 60 bundled skills, SKILL.md parser, FangHub marketplace |
| openfang-hands | 7 autonomous Hands, HAND.toml parser, lifecycle management |
| openfang-extensions | 25 MCP templates, AES-256-GCM credential vault, OAuth2 PKCE |
| openfang-wire | OFP P2P protocol with HMAC-SHA256 mutual auth |
| openfang-cli | CLI, daemon management, TUI dashboard, MCP server mode |
| openfang-desktop | Tauri 2.0 native app |
| openfang-migrate | OpenClaw/LangChain/AutoGPT config migration (no DB interaction) |
| xtask | Build automation |

### Memory layer stores (all on SurrealDB)

| Store | File | Purpose |
|-------|------|---------|
| Sessions | session.rs | Conversation state and history |
| Semantic | semantic.rs | Vector embeddings + text memories |
| Knowledge | knowledge.rs | Entity graph (RELATE) |
| Structured | structured.rs | Agent state/config (double-serialized — P2 fix) |
| Usage | usage.rs | Token/cost metering |
| Consolidation | substrate.rs | Decay/prune/merge (currently no-op) |

### Boot sequence

1. Config loaded from `openfang.toml` (or defaults)
2. `MemorySubstrate::open(db_path)` calls `db::init()`
3. `db::init()` creates `Surreal::new::<SurrealKv>(path)`, sets ns/db
4. **No DDL executed** — schemaless auto-creation
5. Kernel starts, agent loop begins
6. Embedding model auto-detected (dimension unknown at init time)

### Key files for the hardening plan

| File | What to change |
|------|---------------|
| `crates/openfang-memory/src/db.rs` | Add all DEFINE TABLE/INDEX/ANALYZER DDL |
| `crates/openfang-memory/src/session.rs:461-469` | Wrap write_jsonl_mirror in spawn_blocking |
| `crates/openfang-memory/src/semantic.rs:199-222` | Replace brute-force cosine with HNSW KNN |
| `crates/openfang-memory/src/knowledge.rs:116` | Implement max_depth in graph traversal |
| `crates/openfang-memory/src/substrate.rs:595-604` | Wire consolidate() to iterate agents |
| `crates/openfang-memory/src/structured.rs:37-38` | Refactor agent double-serialization |
| `crates/openfang-types/src/config.rs:1500` | Rename sqlite_path to db_path |
| `crates/openfang-kernel/src/kernel.rs:561` | Update sqlite_path reference |
| `README.md:197` | Update "SQLite + vector" to "SurrealDB (multi-model)" |

---

## 8. Burn / Candle Integration Path

### For baked-in embeddings (MiniLM, 384 dimensions)

**Burn** (tracel-ai):
- `minilm-burn` crate exists for sentence embeddings
- Pure Rust, no unsafe, backend-agnostic (CUDA/Metal/WGPU/CPU)
- Apache-2.0 / MIT dual license
- `tracel-ai/models` repo has MiniLM, BERT, Llama

**Candle** (HuggingFace):
- Broader model support, direct Hub integration
- CUDA/Metal support
- Apache-2.0 license
- Would need to port or use candle-transformers for embeddings

### For baked-in STT/TTS

**Whisper:** Available in both Burn and Candle
**Kokoro:** Would need ONNX export or custom implementation

### Integration approach

New crate: `openfang-embedding` (or add to openfang-memory)
- Wraps Burn/Candle with MiniLM
- Fixed 384-dim output
- GPU auto-selection at runtime
- No external API calls needed
- Dimension known at compile time → DDL can hardcode 384

---

## 9. WebRTC-rs Integration Path

**webrtc-rs:** Pure Rust WebRTC implementation
- Replaces the current VICE voice pipeline (separate service on port 7700)
- Handles: peer connection, media streaming, data channels
- Integrates with baked-in STT/TTS for real-time voice

### Current voice architecture (RTR cluster)

| Port | Service | Node |
|------|---------|------|
| 7700 | VICE voice pipeline | Projector |
| 7733 | Whisper STT | Projector |
| 7744 | Kokoro TTS | Projector |
| 7745 | TTS shim (ElevenLabs→Kokoro) | Projector |

### Target: all four collapse into the openfang binary via WebRTC-rs + Burn

---

## 10. Gaps Between SurrealDB Capabilities and OpenFang Usage

| SurrealDB Feature | Status in OpenFang | Priority |
|---|---|---|
| Document store | ✅ Used | — |
| Graph (RELATE) | ⚠️ Single-hop only | P1 |
| Vector (HNSW) | ❌ No index defined | P0 |
| Full-text (BM25) | ❌ Not used | P2 |
| Hybrid search | ❌ Not used | Future |
| Time series views | ❌ Not used (usage/metering) | P2 |
| Live queries | ❌ Not used (dashboard) | Future |
| SurrealML (ONNX) | ❌ Not used (embeddings) | Future |
| Schema enforcement | ❌ Fully schemaless | P0 |
| Geospatial | N/A | — |
| WASM plugins | N/A | — |
| Access control | N/A (single-user embedded) | — |
