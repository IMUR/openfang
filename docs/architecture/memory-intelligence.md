# OpenFang Memory Intelligence Architecture

**Date:** 2026-04-01
**Branch:** `surreang` (prtr local, not pushed to origin)
**Version:** 0.5.1

---

## Overview

The memory layer is a multi-model substrate built on embedded SurrealDB (SurrealKV). Every agent has access to six stores that persist across sessions. **Dense embeddings** (vectors for HNSW) can come from **any** provider that implements the kernel's `EmbeddingDriver`: local **Ollama / vLLM / LM Studio** over HTTP, cloud OpenAI-compatible APIs, or **optional in-process Candle** when the binary is built with the `memory-candle` feature.

The design principle: **the agent loop and kernel are unchanged**. `kernel.send_message_streaming()` is unmodified. Voice, text, API, and channel interactions all flow through the same path. The memory layer enriches context silently — the agent receives better recall results without knowing how embeddings were produced.

### Swapping embedding inference and models (config-only)

No rebuild is required to change **model** or **HTTP backend** — edit `~/.openfang/config.toml` and restart the daemon.

| Goal | What to set |
|------|------------------|
| Local Ollama embeddings | `embedding_provider = "ollama"`, `embedding_model = "nomic-embed-text"` (or any model Ollama serves), `[provider_urls]` `ollama = "http://…:11434/v1"` if non-default |
| vLLM / LM Studio / other OpenAI-compatible | `embedding_provider` to the matching name, `embedding_model` to the server's embedding model id, URL under `provider_urls` |
| In-process Candle (CUDA/CPU) | Build with `cargo build -p openfang-cli --release --features memory-candle`, then `embedding_provider = "candle"` and `embedding_model` = HF id (e.g. `BAAI/bge-small-en-v1.5`) |

**HNSW dimension** must match the model output (e.g. 384 for BGE-small, 768 for nomic-embed-text). The SurrealDB DDL in `openfang-memory` defines the index dimension; change provider/model and DDL together when switching embedding size.

**NER + cross-encoder reranking + ML classification** use in-process Candle when the binary includes `memory-candle`. They are **not** tied to the embedding provider: set `ner_backend`, `reranker_backend`, and `classification_backend` to `"candle"` to run them alongside HTTP embeddings. Omit or `auto` keeps legacy behavior. Use `none` to force-disable while keeping model ids in the file.

Health detail includes `memory_candle_binary: true|false` so operators know whether NER/rerank/classification can ever be active.

---

## Machine GPU Allocation

| GPU | VRAM | CC | Role | Status |
|-----|------|----|------|--------|
| GTX 970 #0 | 4GB (3.5GB fast) | 5.2 | Available (memory intelligence moved to CPU) | Idle — can be reassigned |
| GTX 1080 #1 | 8GB | 6.1 | Ollama LLMs | Active |
| GTX 1080 #2 | 8GB | 6.1 | Ollama LLMs | Active |
| GTX 970 #3 | 4GB (3.5GB fast) | 5.2 | **Voice inference** — Kokoro / Whisper / LFM2.5-Audio | Active (see `voice-intelligence.md`) |

Memory intelligence runs on **CPU** (i9-9900X with AVX-512). All Candle BERT models load FP32 in-process with `cuda_device` omitted.

All four GPUs are **Maxwell/Pascal** (pre-Volta, CC 5.2–6.1). None have Tensor Cores. The Candle inference library is patched at `git.ism.la:6666/rtr/candle-kernels` to support pre-Volta hardware.

---

## Memory Substrate: Six Stores

All six stores share one embedded SurrealDB handle backed by SurrealKV at `~/.openfang/data/openfang.db`. No separate database process — every query is a direct in-process function call.

```
~/.openfang/data/openfang.db/   (SurrealKV LSM tree)
├── sessions                     conversation history per agent
├── canonical_sessions           compacted long-term session view
├── memories                     semantic store — embeddings + BM25
│   ├── HNSW index (384d)        approximate nearest-neighbour search
│   └── BM25 index               full-text search (snowball English)
├── entities                     knowledge graph nodes
├── relations  (TYPE RELATION)   knowledge graph edges
├── kv                           structured agent state
└── usage                        token/cost metering
```

### SurrealDB Features

| Feature | Status | How Used |
|---------|--------|----------|
| Document store | Active | All six tables |
| HNSW vector index | Active | `memories.embedding` — 384d cosine, M=16 EFC=200 TYPE F32 |
| BM25 full-text index | Active | `memories.content` — snowball(english) analyzer |
| Graph edges (RELATE) | Active | `entities → relations → entities` |
| Multi-hop traversal (`->`) | Active | `traverse_from()` up to 3 hops |
| Secondary indexes | Active | `agent_id` on memories / sessions / kv / usage |
| MVCC / time-travel | Available | SurrealKV engine — not yet used |
| Live queries | Available | Not yet used |
| SurrealML | Blocked | Three-way `ort`/`ndarray`/`surrealdb` version conflict — requires `surrealdb` 2.7+ |

---

## Candle Inference Backend (CPU)

Four BERT-family models run in-process via HuggingFace Candle (0.10.0) on the **CPU** (FP32). Models download automatically on first boot to `~/.openfang/models/{model_id}/`. Set `cuda_device = <ordinal>` to use a GPU instead (FP16).

### Active Models

| Model | Params | RAM (FP32, CPU) | VRAM (FP16, GPU) | Task |
|-------|--------|-----------------|------------------|------|
| `BAAI/bge-small-en-v1.5` | 33M | ~133MB | ~85MB | Sentence embeddings (384d) |
| `dslim/bert-base-NER` | 110M | ~433MB | ~270MB | Named entity extraction |
| `cross-encoder/ms-marco-MiniLM-L-6-v2` | 22M | ~91MB | ~60MB | HNSW candidate reranking |
| `typeform/distilbert-base-uncased-mnli` | 67M | ~135MB | ~90MB | Zero-shot NLI memory classification |
| **Total** | **232M** | **~792MB** | **~505MB** (+ ~300MB CUDA ctx) | |

Currently running on **CPU (FP32)**. GPU column retained for reference if `cuda_device` is restored.

The kernel holds optional `Arc<>` handles (`ner_driver`, `reranker`, `classifier`) according to config and `MemoryConfig::wants_candle_*`. These are **independent of the embedding provider** — you can use HTTP embeddings with any combination of Candle NER/rerank/classify, or all four on Candle, or any mix.

### CUDA Compatibility Patch

Upstream `candle-kernels` 0.10.0 unconditionally compiled MoE Tensor Core kernels (`moe_wmma.cu`, `moe_wmma_gguf.cu`) that require CC ≥ 7.0. All four GPUs here are pre-Volta.

**Fix:** `git.ism.la:6666/rtr/candle-kernels` — one `build.rs` commit:
- CC ≥ 7.0: compile all three MoE files (upstream behavior)
- CC < 7.0: compile only `moe_gguf.cu` + empty linker stubs for the Tensor Core symbols (never called for BERT inference)

```toml
# Cargo.toml
[patch.crates-io]
candle-kernels = { git = "ssh://git@git.ism.la:6666/rtr/candle-kernels.git", branch = "main" }
```

---

## Agent Interface to Memory

Agents interact with the memory system through **three distinct layers**. Understanding which layer does what matters when debugging context issues or extending agent behaviour.

### Layer 1 — Automatic Pre-Turn Injection (transparent, semantic store)

At the start of every turn, before the LLM is called, `agent_loop.rs` automatically runs a full recall pipeline against the user's message and appends the results to the system prompt as a `## Memory` section. The agent never explicitly requests this — it just sees recalled memories as part of its instructions.

The recall pipeline in order:
1. Embed the user message → HNSW KNN search (falls back to BM25 text search if no embedding driver)
2. Cross-encoder reranking of candidates (`memory-candle`)
3. Graph-boosted re-ordering via NER entity overlap (`memory-candle`)
4. Scope-weighted stable-sort (`semantic` first, then `declarative`, then `episodic`)
5. Top-5 fragments injected via `prompt_builder::build_memory_section()`

This layer queries the **semantic store** (`memories` table — HNSW + BM25). It is assembled in `crates/openfang-runtime/src/prompt_builder.rs`.

### Layer 2 — Explicit Tool Calls (agent-driven, KV store)

Agents are granted up to four memory tools depending on their manifest capabilities:

| Tool | Store | Operation |
|------|-------|-----------|
| `memory_store(key, value)` | **KV store** (`kv` table) | Write a structured key/value fact |
| `memory_recall(key)` | **KV store** (`kv` table) | Read a specific key by name |
| `memory_delete(key)` | **KV store** (`kv` table) | Remove a key |
| `memory_list` | **KV store** (`kv` table) | Enumerate stored keys |

**Critical distinction:** tool-driven memory targets the **KV store** (`structured_get` / `structured_set`), not the semantic store. The automatic pre-turn injection (Layer 1) targets the **semantic store** (HNSW/BM25). These are two completely separate SurrealDB tables with different access patterns. An agent can `memory_store` a fact via tool call and then see it surfaced automatically next turn via vector recall only if it was also written to the semantic store during the agent loop's write path — the KV store is not vectorised.

Tool calls are capability-checked against `MemoryRead(key)` / `MemoryWrite(key)` before execution. Implementation lives in `crates/openfang-runtime/src/host_functions.rs` → `KernelHandle`.

Typical use: agents use `memory_store` to persist structured user preferences (`user_name`, `preferred_language`, etc.) that they want to look up by exact key in future sessions.

### Layer 3 — Static Workspace Context Files (loaded at session build time)

When a session is built, the kernel loads a set of markdown files from the agent's workspace directory (`~/.openfang/workspaces/<workspace>/`) and injects them as static sections in the system prompt via `PromptContext`. These are loaded once — not queried per turn.

| File | System prompt section | Purpose |
|------|-----------------------|---------|
| `SOUL.md` | `## Persona` | Tone and personality |
| `IDENTITY.md` | `## Identity` | At-a-glance personality summary |
| `USER.md` | `## User Context` | Static facts about the user |
| `MEMORY.md` | `## Long-Term Memory` | Curated persistent knowledge |
| `AGENTS.md` | injected inline | Behavioural guidelines |
| `BOOTSTRAP.md` | `## First-Run Protocol` | First-session ritual (suppressed once `user_name` is known) |

All three layers are assembled in `crates/openfang-runtime/src/prompt_builder.rs` → `build_system_prompt()`. The canonical context summary (session compaction) is injected as a separate user-turn message rather than in the system prompt, to preserve provider prompt-cache hits across turns.

---

## Data Flows

### Write Path (per conversation turn)

```
Agent Loop (agent_loop.rs)
    │
    ├─ 1. Session write ──────────────────────────► sessions (SurrealDB)
    │
    ├─ 2. Rule-based classification
    │       classify_memory(content, source)
    │           → (scope, category, priority)        e.g. "episodic", "observation", "normal"
    │
    ├─ 2b. ML classification (Phase 5, if classifier loaded)
    │       CandleClassifier.classify(content)        [cfg(feature = "memory-candle")]
    │           (spawn_blocking → CPU, FP32)
    │           DistilBERT NLI: content vs. scope/category hypotheses
    │           → overrides rule-based (scope, category) when available
    │           classification_source = "candle" | "rule"
    │
    ├─ 3. Embedding
    │       EmbeddingDriver.embed_one(text)
    │           (HTTP: Ollama/vLLM/OpenAI) OR (Candle: spawn_blocking → CPU, FP32)
    │           → Vec<f32> 384d
    │
    ├─ 4. Memory write ───────────────────────────► memories (SurrealDB)
    │       content + embedding + metadata (session_id, turn_index, source_role,
    │       token_count, scope, category, priority, classification_source)
    │       → HNSW auto-indexes, BM25 auto-indexes
    │
    ├─ 5. AfterMemoryStore hook fired
    │
    ├─ 6. User directive detection (Phase 1d)
    │       is_user_directive(text) → store additional declarative memory
    │           scope = "declarative", source = UserProvided
    │
    ├─ 7. Tool result memories (Phase 4b)
    │       Successful tool executions stored as procedural memories
    │           scope = "procedural", source = Observation
    │
    └─ 8. NER extraction (post-loop, best-effort)
            kernel.populate_knowledge_graph(agent_id, response_text, memory_id)
                CandleNerDriver.extract_entities(text)   [cfg(feature = "memory-candle")]
                    (spawn_blocking → CPU, FP32)
                    BertModel + token-classification head → BIO tags → entity spans
                        knowledge.add_entity()   → entities (SurrealDB)
                        knowledge.add_relation() → relations (SurrealDB)
                    set_metadata_entities(memory_id, entity_ids)
            Streaming: spawned as background tokio task after loop returns
            Non-streaming: inline await after loop returns
```

### Read Path (per recall query)

```
Recall Query (agent_loop.rs, top of run_agent_loop / run_agent_loop_streaming)
    │
    ├─ EmbeddingDriver.embed_one(query) → Vec<f32> 384d
    │
    ├─ Parallel SurrealDB queries
    │   ├─ sessions:   ORDER BY accessed_at DESC
    │   ├─ memories:   HNSW KNN <|20,40|> $query_embedding  (oversample 3×)
    │   │              post-filter: deleted=false, agent_id
    │   └─ knowledge:  traverse_from(entity_id, max_depth=2)
    │
    ├─ BM25 (when query has text)
    │   memories: content @1@ $query_text → search::score(1)
    │
    ├─ Hybrid merge (vector + text)
    │   weighted score: 0.6 × vector_rank + 0.4 × text_rank
    │   dedup by record ID, sort, truncate
    │
    ├─ Cross-encoder reranking (Phase 2, if reranker loaded)   [cfg(feature = "memory-candle")]
    │       CandleReranker.rerank(query, candidates)
    │           (spawn_blocking → CPU, FP32)
    │           BertModel (query, doc) pairs → [CLS] → Linear(1) → score
    │           sorted descending → MemoryFragment[]
    │
    ├─ Graph-boosted recall (Phase 2b)   [cfg(feature = "memory-candle")]
    │       CandleNerDriver.extract_entities(query, threshold=0.60)
    │           query entity names matched against memories.metadata.entities
    │           memories with entity overlap sorted to front
    │
    ├─ Scope-weighted recall (Phase 2c, unconditional)
    │       stable-sort: semantic(0) > declarative(1) > others(2)
    │       information-dense summaries and user facts surface first
    │
    └─ AfterMemoryRecall hook fired
```

### Consolidation Path (background, periodic)

```
Kernel background loop (start_background_agents)
    │
    ├─ ConsolidationEngine.run_for_all_agents()
    │   │
    │   ├─ SELECT DISTINCT agent_id FROM memories WHERE deleted = false
    │   │
    │   └─ Per agent:
    │           decay_confidence()  confidence × (1 − 0.05)
    │               WHERE accessed_at < now − 7 days
    │           prune()  SET deleted = true
    │               WHERE confidence < 0.1
    │
    ├─ L1 Summarization (Phase 3a)
    │       kernel.run_l1_summarization(agent_id)
    │           fetch_episodic_for_summarization() → EpisodicBatchItem[]
    │               (episodic memories older than 24h, not yet summarized)
    │           LLM prompt: "Summarize these N interactions..."
    │           write_l1_summary(agent_id, summary_text, source_ids)
    │               → stored as semantic memory, scope="semantic", category="observation"
    │           mark_memories_summarized(source_ids, summary_id)
    │               → confidence reduced, metadata.summarized=true
    │
    └─ Session summaries (Phase 4d)
            try_generate_session_summary(agent_id, session)
                triggered after every N turns (configurable)
                LLM prompt: "Summarize this conversation..."
                → stored as semantic memory
```

---

## Memory Scope Vocabulary

All memories carry a `scope` field that drives weighted recall.

| Scope | Source | Written By | Description |
|-------|--------|-----------|-------------|
| `episodic` | Conversation | Agent loop (default) | Raw conversation turns |
| `declarative` | UserProvided | User directive detection | Explicit user facts ("remember that...") |
| `semantic` | System/Inference | L1 consolidation, session summaries | Distilled knowledge |
| `procedural` | Tool results | Tool execution path | How-to knowledge from tool use |

Scope constants live in `openfang_types::memory::scope`.

---

## Embedding Latency

| Path | Latency | Notes |
|------|---------|-------|
| Previous: Ollama HTTP | ~887ms | Network + serialization + model load |
| Candle CUDA (healthy) | ~sub-ms–few ms | In-VRAM FP16; first call pays mmap/load |
| Candle CPU (`cuda_device` omitted) | ~5–15ms typical | In-process; depends on CPU (e.g. AVX-512) |
| **No embedding driver** (Candle init failed) | — | Recall uses BM25 / text / recent paths only — **no new vector writes** until a working embedding driver is restored |

BERT weights load at driver init (boot), not lazily on first `remember()`.

---

## Kernel Integration

```
OpenFangKernel
├─ embedding_driver: Option<Arc<dyn EmbeddingDriver>>
│       CandleEmbeddingDriver — provider="candle", model="BAAI/bge-small-en-v1.5"
│
├─ ner_driver: Option<Arc<CandleNerDriver>>          [cfg(feature = "memory-candle")]
│       loaded at boot when ner_backend = "candle"
│       used in populate_knowledge_graph() and graph-boosted recall
│
├─ reranker: Option<Arc<CandleReranker>>             [cfg(feature = "memory-candle")]
│       loaded at boot when reranker_backend = "candle"
│       passed into run_agent_loop / run_agent_loop_streaming
│
├─ classifier: Option<Arc<CandleClassifier>>         [cfg(feature = "memory-candle")]
│       loaded at boot when classification_backend = "candle"
│       zero-shot NLI: DistilBERT-MNLI classifies scope + category at write time
│       falls back to rule-based when classifier = None or inference fails
│
└─ populate_knowledge_graph(agent_id, text, memory_id)
        called after agent loop returns (streaming: background spawn; non-streaming: inline)
```

All four Candle handles are `Option` — the system degrades gracefully if a model fails to load.

| Subsystem | If `None` / inactive |
|-----------|----------------------|
| `embedding_driver` | No dense vectors on write; recall skips the HNSW leg (BM25 + structured paths still work). |
| `ner_driver` | No automatic entity/relation extraction; no graph-boosted recall scoring. |
| `reranker` | HNSW/BM25 ordering unchanged (no cross-encoder rescoring). |
| `classifier` | Rule-based classification used instead (scope from source role, fixed category/priority). |

---

## Hook Events

Two hook events fire on every memory operation, enabling custom observability and extensions.

| Event | When | Payload |
|-------|------|---------|
| `AfterMemoryStore` | After each interaction is written to SurrealDB | `memory_id`, `scope`, `source`, `category` |
| `AfterMemoryRecall` | After recall results are returned | `query`, `result_count`, `retrieval_path` |

---

## Backfill CLI

Existing memories written before classification/NER were active can be re-enriched without a full reset:

```bash
# Dry run — report what would be updated
openfang memory backfill --dry-run

# Backfill all agents
openfang memory backfill

# Backfill a single agent
openfang memory backfill --agent <uuid>

# Scripting
openfang memory backfill --json
```

API: `POST /api/memory/backfill` — body `{ "agent_id": "...", "dry_run": false, "batch_size": 50 }`.

The backfill pipeline:
1. Paginates all non-deleted fragments via `MemorySubstrate::list_fragments`
2. Applies rule-based scope/category if not already ML-classified
3. Upgrades to Candle ML classification if `classifier` is loaded
4. Populates empty `entities` metadata via NER if `ner_driver` is loaded
5. Writes updated metadata via `update_metadata`

---

## Health & Observability

Authenticated **GET** `/api/health/detail` includes a `memory_intelligence` object:

| Field | Meaning |
|-------|---------|
| `embedding_provider` | Config string, e.g. `candle` |
| `embedding_model` | Configured model id |
| `embedding_active` | `true` only if `embedding_driver` initialized |
| `ner_backend` | Raw config value: `auto`, `candle`, `none`, etc. |
| `reranker_backend` | Raw config value: `auto`, `candle`, `none`, etc. |
| `ner_active` | NER model loaded |
| `reranker_active` | Cross-encoder loaded |
| `memory_candle_binary` | `true` if compiled with `--features memory-candle` |
| `cuda_device` | Config value (JSON `null` if unset — CPU mode) |
| `models_cached` | Heuristic: `~/.openfang/models/<embedding_model>/model.safetensors` exists |

> **Note:** `classification_backend` and `classifier_active` are not yet surfaced by the health endpoint despite being wired in the kernel. They should be added to the `memory_intelligence` block in `server.rs` when the health response is next revised.

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

All four subsystems active on CPU. Verify via `/api/health/detail` (actual response shape):
```json
"memory_intelligence": {
  "embedding_provider": "candle",
  "embedding_model": "BAAI/bge-small-en-v1.5",
  "embedding_active": true,
  "ner_backend": "candle",
  "reranker_backend": "candle",
  "ner_active": true,
  "reranker_active": true,
  "memory_candle_binary": true,
  "cuda_device": null,
  "models_cached": true
}
```

`classifier_active` / `classification_backend` are not yet in the response — see note in Health & Observability section above.

---

## Phase Completion Status

| Phase | Description | Status |
|-------|-------------|--------|
| 0 | KG wiring — real MemoryId, entity back-links, traverse_from | ✅ Complete |
| 1 | Rich write path — scope vocab, metadata, rule-based classification, user directives | ✅ Complete |
| 2 | Graph-boosted recall — NER on query, entity overlap scoring, scope-weighted sort | ✅ Complete |
| 3 | Tiered memory — L1 consolidation summarization, session summaries | ✅ Complete |
| 4 | Memory automation — AfterMemoryStore/Recall hooks, tool result memories | ✅ Complete |
| 5 | Candle classification driver — zero-shot NLI override at write time | ✅ Complete |
| Backfill | CLI + API to re-enrich existing memories | ✅ Complete |

---

## Phase 2 (Pending)

| Change | What | Blocker |
|--------|------|---------|
| Upgrade embeddings | `nomic-embed-text-v1` (768d) via JinaBert in candle-transformers | HNSW DDL update: 384→768 |
| Upgrade NER | `LiquidAI/LFM2-350M-Extract` ONNX | `protoc` needed for `candle-onnx` |
| Session compaction | `LiquidAI/LFM2-2.6B-Transcript` ONNX Q4 | Same `protoc` requirement |
| SurrealML in-query inference | `ml::` salience scoring + `DEFINE EVENT` auto-tagging | `surrealml-core` ort/ndarray version triangle — needs `surrealdb` 2.7+ |
| Consolidation scheduling | Timer invoking `consolidate()` every N hours | `consolidation_interval_hours` config field exists, no scheduler wired |
| Runtime-swappable inference | Trait-based HTTP/Candle backends for NER, rerank, classify without rebuild | Deferred — all three currently in-process Candle only |

---

## Deployment & Troubleshooting

**Replacing the daemon binary:** stop the user service before copying:

```bash
systemctl --user stop openfang
cp target/release/openfang ~/.local/bin/openfang
systemctl --user start openfang
```

**CUDA init failures** (`CUDA_ERROR_NOT_FOUND`, "named symbol not found" at `BertModel::load`): usually driver/runtime mismatch. Confirm `nvidia-smi`, rebuild after driver upgrades, or omit `cuda_device` to run on CPU.

**NER tokenizer:** some HF repos (e.g. `dslim/bert-base-NER`) omit `tokenizer.json`. `model_cache::resolve_model` falls back to downloading from `bert-base-cased` when the primary repo returns 404. You can place a file manually under `~/.openfang/models/<model_id>/tokenizer.json`.

**Cargo / SSH:** workspace `.cargo/config.toml` sets `git-fetch-with-cli = true` so `[patch]` git deps fetch via the system `git` + SSH agent.

---

## Repository State

| Location | Contents |
|----------|----------|
| `surreang` branch (typical) | Memory + Candle integration on prtr |
| `git.ism.la:6666/rtr/candle-kernels` | Forked `candle-kernels` with pre-Volta CC guard |
| `~/.openfang/config.toml` | Live daemon config |
| `~/.openfang/data/openfang.db/` | SurrealKV data (live) |
| `~/.openfang/models/` | HF model cache |

Long-lived feature branches may **diverge from** `origin/main`; merge or cherry-pick upstream fixes deliberately rather than blind fast-forward.
