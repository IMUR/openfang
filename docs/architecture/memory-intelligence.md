# OpenFang Memory Intelligence Architecture

**Date:** 2026-04-20
**Branch:** `main` (Forgejo `rtr/openfang`)
**Version:** 0.6.0

---

## Overview

The memory layer is a multi-model substrate built on embedded SurrealDB 3.0.5 (SurrealKV 0.21.0). Every agent has access to six stores that persist across sessions. **Dense embeddings** (vectors for HNSW) can come from **any** provider that implements the kernel's `EmbeddingDriver`: local **Ollama / vLLM / LM Studio** over HTTP, cloud OpenAI-compatible APIs, or **in-process Candle** (active on this deployment — four BERT-family models running on CPU via the `memory-candle` feature flag).

The design principle: **the agent loop and kernel are unchanged**. `kernel.send_message_streaming()` is unmodified. Voice, text, API, and channel interactions all flow through the same path. The memory layer enriches context silently — the agent receives better recall results without knowing how embeddings were produced.

### Swapping embedding inference and models (config-only)

No rebuild is required to change **model** or **HTTP backend** — edit `~/.openfang/config.toml` and restart the daemon.


| Goal                                       | What to set                                                                                                                                                                     |
| ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Local Ollama embeddings                    | `embedding_provider = "ollama"`, `embedding_model = "nomic-embed-text"` (or any model Ollama serves), `[provider_urls]` `ollama = "http://...:11434/v1"` if non-default         |
| vLLM / LM Studio / other OpenAI-compatible | `embedding_provider` to the matching name, `embedding_model` to the server's embedding model id, URL under `provider_urls`                                                      |
| In-process Candle (CUDA/CPU)               | Build with `cargo build -p openfang-cli --release --features memory-candle`, then `embedding_provider = "candle"` and `embedding_model` = HF id (e.g. `BAAI/bge-small-en-v1.5`) |


**HNSW dimension** must match the model output (e.g. 384 for BGE-small, 768 for nomic-embed-text). The SurrealDB DDL in `openfang-memory` defines the index dimension; change provider/model and DDL together when switching embedding size.

**NER + cross-encoder reranking + ML classification** use in-process Candle when the binary includes `memory-candle`. They are **not** tied to the embedding provider: set `ner_backend`, `reranker_backend`, and `classification_backend` to `"candle"` to run them alongside HTTP embeddings. Omit or `auto` keeps legacy behavior. Use `none` to force-disable while keeping model ids in the file.

Health detail includes `memory_candle_binary: true|false` so operators know whether NER/rerank/classification can ever be active.

---

## Machine GPU Allocation

### prtr (Projector)


| GPU         | VRAM             | CC  | Role                                  | Status |
| ----------- | ---------------- | --- | ------------------------------------- | ------ |
| GTX 970 #0  | 4GB (3.5GB fast) | 5.2 | Unassigned                            | Idle   |
| GTX 1080 #1 | 8GB              | 6.1 | Freed (prev. Whisper+Kokoro, retired) | Idle   |
| GTX 1080 #2 | 8GB              | 6.1 | Ollama LLMs                           | Active |
| GTX 970 #3  | 4GB (3.5GB fast) | 5.2 | Unassigned                            | Idle   |


### drtr (Director)


| GPU      | VRAM | CC           | Role                                                   | Status              |
| -------- | ---- | ------------ | ------------------------------------------------------ | ------------------- |
| RTX 2080 | 8GB  | 7.5 (Turing) | **Voice pipeline** (Chatterbox 3.2GB + Parakeet 1.4GB) | Active, ~4.6GB used |


Memory intelligence runs on **CPU** (i9-9900X with AVX-512). All Candle BERT models load FP32 in-process with `cuda_device` omitted.

All four prtr GPUs are **Maxwell/Pascal** (pre-Volta, CC 5.2-6.1). None have Tensor Cores. Candle CUDA is compile-time disabled (`candle-core` built without the `cuda` feature). The `rtr/candle-kernels` patch repo on Forgejo still exists but is no longer referenced in `Cargo.toml`.

---

## Memory Substrate: Six Stores

All six stores share one embedded SurrealDB 3.0.5 handle backed by SurrealKV at `~/.openfang/data/openfang.db`. No separate database process — every query is a direct in-process function call.

```
~/.openfang/data/openfang.db/   (SurrealKV 0.21.0 LSM tree)
+-- sessions                     conversation history per agent
+-- canonical_sessions           compacted long-term session view
+-- memories                     semantic store -- embeddings + BM25
|   +-- HNSW index (384d)        approximate nearest-neighbour search
|   +-- BM25 index               full-text search (snowball English)
+-- entities                     knowledge graph nodes
+-- relations  (TYPE RELATION)   knowledge graph edges
+-- kv                           structured agent state
+-- usage                        token/cost metering
+-- paired_devices               mobile device pairing
+-- task_queue                   A2A task distribution
```

### SurrealDB 3.0 Features


| Feature                    | Status    | How Used                                                    |
| -------------------------- | --------- | ----------------------------------------------------------- |
| Document store             | Active    | All tables (SCHEMALESS)                                     |
| HNSW vector index          | Active    | `memories.embedding` — 384d cosine, M=16 EFC=200 TYPE F32   |
| BM25 full-text index       | Active    | `memories.content` — FULLTEXT ANALYZER, snowball(english)   |
| Graph edges (RELATE)       | Active    | `entities -> relations -> entities`                         |
| Multi-hop traversal (`->`) | Active    | `traverse_from()` up to 3 hops                              |
| Secondary indexes          | Active    | `agent_id` on memories / sessions / kv / usage              |
| MVCC / time-travel         | Available | SurrealKV engine — not yet used                             |
| Live queries               | Available | Not yet used                                                |
| SurrealML                  | Untested  | Version triangle may be resolved by v3 — needs verification |


### SurrealDB v3 DDL Changes

The upgrade from SurrealDB 2.6.5 to 3.0.5 required several DDL syntax changes:


| v2 Syntax                             | v3 Syntax                    | Reason                                           |
| ------------------------------------- | ---------------------------- | ------------------------------------------------ |
| `FLEXIBLE TYPE any`                   | `TYPE any`                   | `FLEXIBLE` restricted to object-containing types |
| `FLEXIBLE TYPE object`                | `TYPE object FLEXIBLE`       | `FLEXIBLE` moved after `TYPE`                    |
| `SEARCH ANALYZER ... BM25(1.2, 0.75)` | `FULLTEXT ANALYZER ... BM25` | Keyword renamed, BM25 params removed             |
| `type::thing('table', $id)`           | `type::record('table', $id)` | Function renamed                                 |
| `TYPE option<T>`                      | `TYPE option                 | null`                                            |


### SurrealDB v3 Rust SDK Changes

The `surrealdb` crate v3 replaced serde `Deserialize` with the `SurrealValue` trait for `.take()` deserialization. Two patterns are used in `openfang-memory`:

1. `**#[derive(SurrealValue)]**` — for structs containing only primitive types (String, i64, f64, bool, Option of primitives). Used in `structured.rs`, `usage.rs`, `substrate.rs`, `consolidation.rs`.
2. `**surreal_via_json!` macro** — for structs containing types from `openfang-types` (EntityType, RelationType, MemorySource, Message) that cannot derive `SurrealValue` without coupling `openfang-types` to `surrealdb`. The macro implements `SurrealValue` via a serde-json round-trip: `self -> serde_json::Value -> surrealdb::types::Value` and back. Used in `knowledge.rs`, `semantic.rs`, `session.rs`.

---

## Candle Inference Backend (CPU)

Four BERT-family models run in-process via HuggingFace Candle (0.10.2) on the **CPU** (FP32). Models download automatically on first boot to `~/.openfang/models/{model_id}/`. CUDA is compile-time disabled (`candle-core` built without the `cuda` feature) — GPU inference is not available in the current binary.

### Active Models


| Model                                   | Params   | RAM (FP32, CPU) | VRAM (FP16, GPU)               | Task                                |
| --------------------------------------- | -------- | --------------- | ------------------------------ | ----------------------------------- |
| `BAAI/bge-small-en-v1.5`                | 33M      | ~133MB          | ~85MB                          | Sentence embeddings (384d)          |
| `dslim/bert-base-NER`                   | 110M     | ~433MB          | ~270MB                         | Named entity extraction             |
| `cross-encoder/ms-marco-MiniLM-L-6-v2`  | 22M      | ~91MB           | ~60MB                          | HNSW candidate reranking            |
| `typeform/distilbert-base-uncased-mnli` | 67M      | ~135MB          | ~90MB                          | Zero-shot NLI memory classification |
| **Total**                               | **232M** | **~792MB**      | **~505MB** (+ ~300MB CUDA ctx) |                                     |


Currently running on **CPU (FP32)**. GPU column retained for reference if `cuda_device` is restored.

The kernel holds optional `Arc<>` handles (`ner_driver`, `reranker`, `classifier`) according to config and `MemoryConfig::wants_candle_`*. These are **independent of the embedding provider** — you can use HTTP embeddings with any combination of Candle NER/rerank/classify, or all four on Candle, or any mix.

### CUDA Status

CUDA is **compile-time disabled** — `candle-core` is built without the `cuda` feature, so no CUDA kernels are compiled and `candle-kernels` is not a dependency. This eliminates the previous requirement for a pre-Volta kernel patch (`rtr/candle-kernels` on Forgejo). The patch repo still exists but is no longer referenced in `Cargo.toml`.

All four GPUs on prtr are pre-Volta (CC 5.2/6.1) with no Tensor Cores. CPU inference via AVX-512 is 5-15ms per embedding — adequate for the current workload. To re-enable CUDA in the future (e.g. with an SM 7.0+ GPU), add `features = ["cuda"]` back to the `candle-core` workspace dependency.

---

## Agent Interface to Memory

Agents interact with the memory system through distinct contracts. Understanding which contract does what matters when debugging context issues or extending agent behaviour. The canonical dimensions framing lives in `/mnt/ops/canon/openfang/memory-intelligence.md`.

### Context Contract — Automatic Pre-Turn Injection (transparent, semantic store)

At the start of every turn, before the LLM is called, `agent_loop.rs` automatically runs a full recall pipeline against the user's message and appends the results to the system prompt as a `## Memory` section. The agent never explicitly requests this — it just sees recalled memories as part of its instructions.

The recall pipeline in order:

1. Embed the user message -> HNSW KNN search (falls back to BM25 text search if no embedding driver)
2. Cross-encoder reranking of candidates (`memory-candle`)
3. Graph-boosted re-ordering via NER entity overlap (`memory-candle`)
4. Scope-weighted stable-sort (`semantic` first, then `declarative`, then `episodic`)
5. Top-5 fragments injected via `prompt_builder::build_memory_section()` with semantic scope and relative timestamp metadata (e.g., `[Episodic - 3 days ago]`).

This contract queries the **semantic store** (`memories` table — HNSW + BM25). It is assembled in `crates/openfang-runtime/src/prompt_builder.rs` and `agent_loop.rs`.

### Tool Contract — Explicit Tool Calls (agent-driven, KV store)

Agents are granted up to four memory tools depending on their manifest capabilities:


| Tool                       | Store                     | Operation                                      |
| -------------------------- | ------------------------- | ---------------------------------------------- |
| `memory_set(key, value)`   | **KV store** (`kv` table) | Write a structured key/value fact              |
| `memory_get(key)`          | **KV store** (`kv` table) | Read a specific key by name                    |
| `memory_delete(key)`       | **KV store** (`kv` table) | Remove a key                                   |
| `memory_list(namespace)`   | **KV store** (`kv` table) | Enumerate stored keys (`"self"` or `"shared"`) |


**Critical distinction:** tool-driven exact-key memory targets the **KV store** (`structured_get` / `structured_set`), not the semantic store. Automatic pre-turn injection targets the **semantic store** (HNSW/BM25). These are two completely separate SurrealDB tables with different contracts. An agent can `memory_set` a fact via tool call and then see it surfaced automatically next turn via vector recall only if that fact is also represented in the semantic store during the agent loop's write path — the KV store is not vectorised.

#### Namespace Routing

Keys are routed to one of two SurrealDB partitions based on their prefix:


| Key prefix            | Storage partition                | SurrealDB `agent_id`                         |
| --------------------- | -------------------------------- | -------------------------------------------- |
| `self.`*              | Agent's **private** namespace    | Calling agent's `AgentId`                    |
| `shared.`*, bare keys | **Shared** cross-agent namespace | Fixed `00000000-0000-0000-0000-000000000001` |


This is handled by `resolve_memory_namespace(key, caller_id)` in `kernel.rs`. The shared namespace uses the same `AgentId` as `shared_memory_agent_id()` — the fixed UUID `[0..0, 0x01]` that also backs the shared KV namespace visible in `kernel.rs`.

An agent with `memory_write = ["self.*"]` can store `self.user_name` (routed to its own partition) but **cannot** write `shared.config` or bare keys like `global_setting` — the capability check rejects the key before the write reaches SurrealDB.

#### Capability Enforcement

Every tool call passes through `KernelHandle` where the kernel enforces capabilities:

```
tool_runner.rs                          kernel.rs
─────────────                           ──────────
tool_memory_set(input, kernel, id)
  → kernel.memory_store(agent_id, key, value)
                                        1. Parse agent_id → AgentId
                                        2. is_system_principal(caller)?
                                           → yes: skip checks (internal infra)
                                           → no: check MemoryWrite(key) capability
                                              → denied: return Err
                                        3. resolve_memory_namespace(key, caller)
                                           → self.* → caller's AgentId
                                           → other  → shared AgentId
                                        4. memory.structured_set(storage_id, key, value)
```

Capabilities in `agent.toml` use glob-style matching:


| Manifest pattern                            | Grants                                  |
| ------------------------------------------- | --------------------------------------- |
| `memory_write = ["self.*"]`                 | Write to own namespace only             |
| `memory_write = ["*"]`                      | Write to any namespace                  |
| `memory_read = ["self.*", "shared.config"]` | Read own keys + one specific shared key |


The **system principal** (`SYSTEM_AGENT_ID = "00000000-0000-0000-0000-000000000001"`, defined in `kernel_handle.rs`) bypasses per-key capability checks entirely. Internal infrastructure — schedule tools, cron delivery — uses this identity to access shared state without requiring explicit capability grants.

WASM agents go through the same enforcement path: `host_functions.rs` (`host_kv_get` / `host_kv_set`) passes the WASM agent's ID to the `KernelHandle` memory methods.

Implementation: `crates/openfang-runtime/src/tool_runner.rs` (dispatch + tool definitions), `crates/openfang-kernel/src/kernel.rs` (`KernelHandle` impl, `resolve_memory_namespace`, `is_system_principal`).

Typical use: agents use `memory_set` to persist structured user preferences (`self.user_name`, `self.preferred_language`, etc.) that they want to look up by exact key in future sessions.

#### Boot-time KV migrations

The kernel runs idempotent KV migrations at startup from a `tokio::spawn` fired inside `start_background_agents`. Currently one migration is wired:

`migrate_shared_memory_schedules` (in `kernel.rs`) sweeps the shared KV namespace for legacy `__openfang_schedules` entries (pre-cron-scheduler agent schedule tools) and replays them into the upstream cron scheduler. On success it clears the legacy key to `[]` and writes a marker:


| Shared-namespace key               | Value after migration |
| ---------------------------------- | --------------------- |
| `__openfang_schedules`             | `[]` (cleared)        |
| `__openfang_schedules_migrated_v1` | `true`                |


Idempotency is gated on the marker — if `__openfang_schedules_migrated_v1 == true` the migration short-circuits and does nothing. Operators running `memory_list shared` will see both keys; **do not delete the marker** or the legacy sweep will re-run (and would be a no-op now, but future migration versions may assume it's present).

Entries pointing at unresolved agent IDs are skipped and logged at `warn` — the migration succeeds partially rather than aborting. Successful migrations trigger `cron_scheduler.persist()`.

### Filesystem Context Contract — Static Workspace Context Files (loaded at session build time)

When a session is built, the kernel loads a set of markdown files from the agent's workspace directory (`~/.openfang/workspaces/<workspace>/`) and injects them as static sections in the system prompt via `PromptContext`. These are loaded once — not queried per turn.


| File           | System prompt section   | Purpose                                                     |
| -------------- | ----------------------- | ----------------------------------------------------------- |
| `SOUL.md`      | `## Persona`            | Tone and personality                                        |
| `IDENTITY.md`  | `## Identity`           | At-a-glance personality summary                             |
| `USER.md`      | `## User Context`       | Static facts about the user                                 |
| `MEMORY.md`    | `## Long-Term Memory`   | Curated persistent knowledge                                |
| `AGENTS.md`    | injected inline         | Behavioural guidelines                                      |
| `BOOTSTRAP.md` | `## First-Run Protocol` | First-session ritual (suppressed once `user_name` is known) |


All three layers are assembled in `crates/openfang-runtime/src/prompt_builder.rs` -> `build_system_prompt()`. The canonical context summary (session compaction) is injected as a separate user-turn message rather than in the system prompt, to preserve provider prompt-cache hits across turns.

---

## Data Flows

### Write Path (per conversation turn)

```
Agent Loop (agent_loop.rs)
    |
    +- 1. Session write -> sessions (SurrealDB)
    |
    +- 2. Rule-based classification
    |       classify_memory(content, source)
    |           -> (scope, category, priority)        e.g. "episodic", "observation", "normal"
    |
    +- 2b. ML classification (Phase 5, if classifier loaded)
    |       CandleClassifier.classify(content)        [cfg(feature = "memory-candle")]
    |           (spawn_blocking -> CPU, FP32)
    |           DistilBERT NLI: content vs. scope/category hypotheses
    |           -> overrides rule-based (scope, category) when available
    |           classification_source = "candle" | "rule"
    |
    +- 3. Embedding
    |       EmbeddingDriver.embed_one(text)
    |           (HTTP: Ollama/vLLM/OpenAI) OR (Candle: spawn_blocking -> CPU, FP32)
    |           -> Vec<f32> 384d
    |
    +- 4. Memory write -> memories (SurrealDB)
    |       content + embedding + metadata (session_id, turn_index, source_role,
    |       token_count, scope, category, priority, classification_source)
    |       -> HNSW auto-indexes, BM25 auto-indexes
    |
    +- 5. AfterMemoryStore hook fired
    |
    +- 6. User directive detection (Phase 1d)
    |       is_user_directive(text) -> store additional declarative memory
    |           scope = "declarative", source = UserProvided
    |
    +- 7. Tool result memories (Phase 4b)
    |       Successful tool executions stored as procedural memories
    |           scope = "procedural", source = Observation
    |
    +- 8. NER extraction (post-loop, best-effort)
            kernel.populate_knowledge_graph(agent_id, response_text, memory_id)
                CandleNerDriver.extract_entities(text)   [cfg(feature = "memory-candle")]
                    (spawn_blocking -> CPU, FP32)
                    BertModel + token-classification head -> BIO tags -> entity spans
                        knowledge.add_entity()   -> entities (SurrealDB)
                        knowledge.add_relation() -> relations (SurrealDB)
                    set_metadata_entities(memory_id, entity_ids)
            Streaming: spawned as background tokio task after loop returns
            Non-streaming: inline await after loop returns
```

### Read Path (per recall query)

```
Recall Query (agent_loop.rs, top of run_agent_loop / run_agent_loop_streaming)
    |
    +- EmbeddingDriver.embed_one(query) -> Vec<f32> 384d
    |
    +- SurrealDB queries
    |   +- memories:   HNSW KNN (oversample, post-filter: deleted=false, agent_id)
    |   +- memories:   BM25 FULLTEXT content @1@ query_text
    |
    +- Hybrid merge (vector + text)
    |   weighted score: 0.6 x vector_rank + 0.4 x text_rank
    |   dedup by record ID, sort, truncate
    |
    +- Cross-encoder reranking (if reranker loaded)   [cfg(feature = "memory-candle")]
    |       CandleReranker.rerank(query, candidates)
    |           (spawn_blocking -> CPU, FP32)
    |           BertModel (query, doc) pairs -> [CLS] -> Linear(1) -> score
    |           sorted descending -> MemoryFragment[]
    |
    +- Graph-boosted recall (if NER loaded)   [cfg(feature = "memory-candle")]
    |       CandleNerDriver.extract_entities(query, threshold=0.60)
    |           query entity names matched against memories.metadata.entities
    |           memories with entity overlap sorted to front
    |
    +- Scope-weighted recall (unconditional)
    |       stable-sort: semantic(0) > declarative(1) > others(2)
    |       information-dense summaries and user facts surface first
    |
    +- AfterMemoryRecall hook fired
```

### Consolidation Path (background, periodic)

```
Kernel background loop (start_background_agents)
    |
    +- ConsolidationEngine.run_for_all_agents()
    |   |
    |   +- SELECT DISTINCT agent_id FROM memories WHERE deleted = false
    |   |
    |   +- Per agent:
    |           decay_confidence()  confidence x (1 - 0.05)
    |               WHERE accessed_at < now - 7 days
    |           prune()  SET deleted = true
    |               WHERE confidence < 0.1
    |
    +- L1 Summarization (Phase 3a)
    |       kernel.run_l1_summarization(agent_id)
    |           fetch_episodic_for_summarization() -> EpisodicBatchItem[]
    |               (episodic memories older than 24h, not yet summarized)
    |           LLM prompt: "Summarize these N interactions..."
    |           write_l1_summary(agent_id, summary_text, source_ids)
    |               -> stored as semantic memory, scope="semantic", category="observation"
    |           mark_memories_summarized(source_ids, summary_id)
    |               -> confidence reduced, metadata.summarized=true
    |
    +- Session summaries (Phase 4d)
            try_generate_session_summary(agent_id, session)
                triggered after every N turns (configurable)
                LLM prompt: "Summarize this conversation..."
                -> stored as semantic memory
```

---

## Memory Scope Vocabulary

All memories carry a `scope` field that drives weighted recall. This scope (along with relative timestamps) is injected directly into the agent's prompt during context-contract pre-turn injection, allowing the LLM to differentiate between its own raw conversation history and curated facts.


| Scope         | Source           | Written By                          | Description                              |
| ------------- | ---------------- | ----------------------------------- | ---------------------------------------- |
| `episodic`    | Conversation     | Agent loop (default)                | Raw conversation turns                   |
| `declarative` | UserProvided     | User directive detection            | Explicit user facts ("remember that...") |
| `semantic`    | System/Inference | L1 consolidation, session summaries | Distilled knowledge                      |
| `procedural`  | Tool results     | Tool execution path                 | How-to knowledge from tool use           |


Scope constants live in `openfang_types::memory::scope`.

---

## Embedding Latency


| Path                                  | Latency         | Notes                                                                                                      |
| ------------------------------------- | --------------- | ---------------------------------------------------------------------------------------------------------- |
| Ollama HTTP                           | ~887ms          | Network + serialization + model load                                                                       |
| Candle CUDA (healthy)                 | ~sub-ms-few ms  | In-VRAM FP16; first call pays mmap/load                                                                    |
| Candle CPU (`cuda_device` omitted)    | ~5-15ms typical | In-process; depends on CPU (e.g. AVX-512)                                                                  |
| **No embedding driver** (init failed) | --              | Recall uses BM25 text search only -- **no new vector writes** until a working embedding driver is restored |


BERT weights load at driver init (boot), not lazily on first `remember()`.

---

## Kernel Integration

```
OpenFangKernel
+- embedding_driver: Option<Arc<dyn EmbeddingDriver>>
|       CandleEmbeddingDriver -- provider="candle", model="BAAI/bge-small-en-v1.5"
|
+- ner_driver: Option<Arc<CandleNerDriver>>          [cfg(feature = "memory-candle")]
|       loaded at boot when ner_backend = "candle"
|       used in populate_knowledge_graph() and graph-boosted recall
|
+- reranker: Option<Arc<CandleReranker>>             [cfg(feature = "memory-candle")]
|       loaded at boot when reranker_backend = "candle"
|       passed into run_agent_loop / run_agent_loop_streaming
|
+- classifier: Option<Arc<CandleClassifier>>         [cfg(feature = "memory-candle")]
|       loaded at boot when classification_backend = "candle"
|       zero-shot NLI: DistilBERT-MNLI classifies scope + category at write time
|       falls back to rule-based when classifier = None or inference fails
|
+- capabilities: CapabilityManager
|       enforces MemoryRead / MemoryWrite per agent per key
|       consulted by memory_set, memory_get, memory_list, memory_delete
|
+- populate_knowledge_graph(agent_id, text, memory_id)
        called after agent loop returns (streaming: background spawn; non-streaming: inline)

KernelHandle (trait, defined in openfang-runtime/src/kernel_handle.rs)
+- SYSTEM_AGENT_ID: &str = "00000000-0000-0000-0000-000000000001"
|       system principal — bypasses per-key capability checks
|
+- memory_set(agent_id, key, value)      capability-checked, namespace-routed
+- memory_get(agent_id, key)             capability-checked, namespace-routed
+- memory_list(agent_id, namespace)       capability-checked, namespace = "self" | "shared"
+- memory_delete(agent_id, key)          capability-checked, namespace-routed

Helper functions (kernel.rs, module-level):
+- resolve_memory_namespace(key, caller_id) -> AgentId
|       "self.*" -> caller_id (private partition)
|       anything else -> shared_memory_agent_id() (cross-agent partition)
|
+- is_system_principal(id) -> bool
        id == shared_memory_agent_id()
```

All four Candle handles are `Option` — the system degrades gracefully if a model fails to load.


| Subsystem          | If `None` / inactive                                                                       |
| ------------------ | ------------------------------------------------------------------------------------------ |
| `embedding_driver` | No dense vectors on write; recall skips the HNSW leg (BM25 + structured paths still work). |
| `ner_driver`       | No automatic entity/relation extraction; no graph-boosted recall scoring.                  |
| `reranker`         | HNSW/BM25 ordering unchanged (no cross-encoder rescoring).                                 |
| `classifier`       | Rule-based classification used instead (scope from source role, fixed category/priority).  |


---

## Hook Events

Two hook events fire on every memory operation, enabling custom observability and extensions.


| Event               | When                                           | Payload                                    |
| ------------------- | ---------------------------------------------- | ------------------------------------------ |
| `AfterMemoryStore`  | After each interaction is written to SurrealDB | `memory_id`, `scope`, `source`, `category` |
| `AfterMemoryRecall` | After recall results are returned              | `query`, `result_count`, `retrieval_path`  |


---

## Backfill CLI

Existing memories written before classification/NER were active can be re-enriched without a full reset:

```bash
# Dry run -- report what would be updated
openfang memory backfill --dry-run

# Backfill all agents
openfang memory backfill

# Backfill a single agent
openfang memory backfill --agent <uuid>

# Scripting
openfang memory backfill --json
```

API: `POST /api/memory/backfill` -- body `{ "agent_id": "...", "dry_run": false, "batch_size": 50 }`.

The backfill pipeline:

1. Paginates all non-deleted fragments via `MemorySubstrate::list_fragments`
2. Applies rule-based scope/category if not already ML-classified
3. Upgrades to Candle ML classification if `classifier` is loaded
4. Populates empty `entities` metadata via NER if `ner_driver` is loaded
5. Writes updated metadata via `update_metadata`

---

## Health & Observability

Authenticated **GET** `/api/health/detail` includes a `memory_intelligence` object:


| Field                    | Meaning                                                                    |
| ------------------------ | -------------------------------------------------------------------------- |
| `embedding_provider`     | Config string, e.g. `candle`                                               |
| `embedding_model`        | Configured model id                                                        |
| `embedding_active`       | `true` only if `embedding_driver` initialized                              |
| `ner_backend`            | Raw config value: `auto`, `candle`, `none`, etc.                           |
| `reranker_backend`       | Raw config value: `auto`, `candle`, `none`, etc.                           |
| `classification_backend` | Raw config value: `auto`, `candle`, `none`, etc.                           |
| `ner_active`             | NER model loaded                                                           |
| `reranker_active`        | Cross-encoder loaded                                                       |
| `classifier_active`      | Zero-shot NLI classifier loaded                                            |
| `memory_candle_binary`   | `true` if compiled with `--features memory-candle`                         |
| `cuda_device`            | Config value (JSON `null` if unset -- CPU mode)                            |
| `models_cached`          | Heuristic: `~/.openfang/models/<embedding_model>/model.safetensors` exists |


---

## Active Configuration (`~/.openfang/config.toml`)

```toml
[memory]
embedding_model        = "BAAI/bge-small-en-v1.5"
embedding_provider     = "candle"
# cuda_device          = 0                   # Omitted = CPU; integer = CUDA ordinal for GPU
ner_backend            = "candle"            # auto | candle | none
reranker_backend       = "candle"
classification_backend = "candle"            # auto | candle | none
# ner_model            = "dslim/bert-base-NER"
# reranker_model       = "cross-encoder/ms-marco-MiniLM-L-6-v2"
# classification_model = "typeform/distilbert-base-uncased-mnli"
consolidation_threshold = 10000
decay_rate             = 0.05
```

All four subsystems active on CPU. Verify via `/api/health/detail`.

---

## Phase Completion Status


| Phase             | Description                                                                                                               | Status   |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------- | -------- |
| 0                 | KG wiring -- real MemoryId, entity back-links, traverse_from                                                              | Complete |
| 1                 | Rich write path -- scope vocab, metadata, rule-based classification, user directives                                      | Complete |
| 2                 | Graph-boosted recall -- NER on query, entity overlap scoring, scope-weighted sort                                         | Complete |
| 3                 | Tiered memory -- L1 consolidation summarization, session summaries                                                        | Complete |
| 4                 | Memory automation -- AfterMemoryStore/Recall hooks, tool result memories                                                  | Complete |
| 5                 | Candle classification driver -- zero-shot NLI override at write time                                                      | Complete |
| Backfill          | CLI + API to re-enrich existing memories                                                                                  | Complete |
| SurrealDB v3      | Upgrade from 2.6.5 to 3.0.5 -- DDL, SDK, type system                                                                      | Complete |
| Memory middleware | Capability enforcement, `self.*` namespace routing, `memory_list`/`memory_delete` tools, system principal, WASM alignment | Complete |


### Recent Fixes & Observations

- **Candle Classifier (`distilbert`)**: The `candle_classifier` was updated to dynamically parse the architecture from `config.json` (checking for `"dim"` vs `"hidden_size"`) to support both `BertModel` and `DistilBertModel` wrappers. This resolved the "missing field hidden_size" error on boot.
- **SurrealDB `DISTINCT` Compatibility**: The consolidation backfill query was updated from `SELECT DISTINCT agent_id FROM memories` to `SELECT agent_id FROM memories WHERE deleted = false GROUP BY agent_id` to comply with the embedded SurrealDB engine's parser, resolving `500 Internal Server Error`s during CLI backfills.
- **The "Agent's Perspective"**: Because context-contract semantic recall operates completely transparently before the LLM prompt is assembled, agents interacting with the system often cannot distinguish between raw conversation history and HNSW-surfaced semantic memories. The prompt masks the complexity of the HNSW + Cross-Encoder reranking happening underneath. When auditing the system from the "outside," it may incorrectly appear as if memory is a "flat dump" or that vector search is disabled.

---

## Phase 2 (Pending)


| Change                       | What                                                                       | Blocker                                                                |
| ---------------------------- | -------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| Upgrade embeddings           | `nomic-embed-text-v1` (768d) via JinaBert in candle-transformers           | HNSW DDL update: 384->768                                              |
| Upgrade NER                  | `LiquidAI/LFM2-350M-Extract` ONNX                                          | `protoc` needed for `candle-onnx`                                      |
| Session compaction           | `LiquidAI/LFM2-2.6B-Transcript` ONNX Q4                                    | Same `protoc` requirement                                              |
| SurrealML in-query inference | `ml::` salience scoring + `DEFINE EVENT` auto-tagging                      | Version triangle may be resolved by v3 -- untested                     |
| Consolidation scheduling     | Timer invoking `consolidate()` every N hours                               | `consolidation_interval_hours` config field exists, no scheduler wired |
| Runtime-swappable inference  | Trait-based HTTP/Candle backends for NER, rerank, classify without rebuild | Deferred -- all three currently in-process Candle only                 |


---

## Deployment & Troubleshooting

**Replacing the daemon binary:** stop the user service before copying:

```bash
systemctl --user stop openfang
cp target/release-fast/openfang ~/.local/bin/openfang
systemctl --user start openfang
```

**Config paths:** use absolute paths in `config.toml`, not `~/.openfang`. Rust does not expand tildes. The systemd unit sets `WorkingDirectory=%h/.openfang`, and a literal `~` in the config creates a `~` directory inside `.openfang`.

**Daemon detection:** the daemon writes `~/.openfang/daemon.json` at startup (PID, listen address, version). The CLI reads this file to decide HTTP mode vs in-process mode. If absent, the CLI tries to boot its own kernel and may hit the SurrealKV LOCK file.

**SurrealDB v3 migration:** the SurrealKV on-disk manifest format changed between v2 and v3. A v2 database cannot be opened by v3. Back up `~/.openfang/data/openfang.db/` before upgrading, and start fresh if needed. The v2 backup can be restored by reverting to the v2 binary.

**CUDA init failures** (`CUDA_ERROR_NOT_FOUND`, "named symbol not found" at `BertModel::load`): usually driver/runtime mismatch. Confirm `nvidia-smi`, rebuild after driver upgrades, or omit `cuda_device` to run on CPU.

**NER tokenizer:** some HF repos (e.g. `dslim/bert-base-NER`) omit `tokenizer.json`. `model_cache::resolve_model` falls back to downloading from `bert-base-cased` when the primary repo returns 404. You can place a file manually under `~/.openfang/models/<model_id>/tokenizer.json`.

**Cargo / SSH:** workspace `.cargo/config.toml` sets `git-fetch-with-cli = true` so `[patch]` git deps fetch via the system `git` + SSH agent.

---

## Implicit SQLite Synchrony Manifold

**Discovery date:** 2026-04-07  
**Status:** Primary instance fixed — commit `ad3499e` (2026-04-07)  
**Root cause:** The upstream (RightNow-AI) codebase assumed SQLite's effectively synchronous writes (sub-millisecond). OpenFang replaced SQLite with async SurrealDB. Several places implicitly relied on writes being fast enough not to matter — they introduced variable latency (10ms–2s+) at points that blocked external consumers.

### Primary instance (FIXED — was causing missing WebSocket response)

`**crates/openfang-runtime/src/agent_loop.rs` — `run_agent_loop_streaming`**

After the LLM emitted its final `EndTurn` response, `stream_tx` (the mpsc `Sender` whose closure signals "stream done" to all consumers) was held alive through all post-response persistence:

```
LLM EndTurn → response text ready
    was holding stream_tx...
    ├── memory.save_session_async(session).await      (SurrealDB write, variable latency)
    ├── clf.classify(&interaction_text).await          (Candle classifier, 100ms-2s)
    ├── emb.embed_one(...).await                       (embedding, variable)
    ├── memory.remember_with_embedding_async().await   (SurrealDB write)
    └── AfterMemoryStore / AgentLoopEnd hooks
    return Ok(AgentLoopResult)  ← stream_tx dropped here (too late)
```

All WebSocket, SSE, OpenAI-compat, and TUI consumers wait for `rx.recv() → None` (channel close). The `type: "response"` JSON was only sent **after** `stream_task.await`, which required channel closure. If post-processing exceeded the browser's WebSocket ping timeout (~30-60s), the browser closed the connection before the `response` JSON arrived — requiring a page refresh. Voice had no recovery path.

**Fix (commit `ad3499e`):** `drop(stream_tx)` called immediately after `final_response = text.clone()` at all exit paths (EndTurn, silent/NO_REPLY, MaxTokens, max iterations). Post-response persistence continues in the same async context but no longer holds any consumer hostage. Additionally, the voice TTS task was fixed to continue reading on `ContentComplete(ToolUse)` rather than breaking — tool-use responses now correctly reach TTS.

The fix restored correct behaviour for:

- WebSocket `type: "response"` arriving without page refresh
- Voice spoken replies after tool use
- SSE and OpenAI-compat stream completion timing

### Remaining instances


| Location                                         | Pattern                                                                            | Severity |
| ------------------------------------------------ | ---------------------------------------------------------------------------------- | -------- |
| `tool_runner.rs` MCP branch ~483–496             | `mcp_connections.lock().await` held across MCP I/O (not SurrealDB, but same class) | MEDIUM   |
| `kernel.rs` compaction + immediate `get_session` | Write-then-read ordering assumption                                                | LOW      |


The `agent_loop.rs` silent/NO_REPLY and tool-use iteration paths that previously appeared here are resolved by the same `ad3499e` fix — `drop(stream_tx)` was added to all exit paths.

### Relationship to memory intelligence

The Candle classifier is the most expensive post-stream operation and was **added to OpenFang, not present in upstream**. It was the primary reason the delay was severe enough to exceed browser ping timeouts. The fix decouples response delivery from persistence entirely, so Candle inference latency no longer affects the client experience — all four models remain active at full quality.

---

## Repository State


| Location                                 | Contents                                                                                          |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------- |
| Forgejo `rtr/openfang` (origin)          | Source of truth -- `main` branch, deployed from                                                   |
| GitHub `IMUR/openfang` (github)          | Curated fork of upstream, comparison view                                                         |
| GitHub `RightNow-AI/openfang` (upstream) | Upstream project                                                                                  |
| Forgejo `rtr/candle-kernels`             | Forked `candle-kernels` with pre-Volta CC guard (inactive — no longer referenced in `Cargo.toml`) |
| Forgejo `rtr/viper`                      | Voice pipeline service code (STT + TTS apps, systemd units)                                       |
| `~/.openfang/config.toml`                | Live daemon config                                                                                |
| `~/.openfang/data/openfang.db/`          | SurrealKV 0.21.0 data (live)                                                                      |
| `~/.openfang/models/`                    | HF model cache                                                                                    |


