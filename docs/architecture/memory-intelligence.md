# OpenFang Memory Intelligence Architecture

**Date:** 2026-04-01
**Branch:** `surreang` (prtr local, not pushed to origin)
**Version:** 0.5.1

---

## Overview

The memory layer is a multi-model substrate built on embedded SurrealDB (SurrealKV). Every agent has access to six stores that persist across sessions. **Dense embeddings** (vectors for HNSW) can come from **any** provider that implements the kernel’s `EmbeddingDriver`: local **Ollama / vLLM / LM Studio** over HTTP, cloud OpenAI-compatible APIs, or **optional in-process Candle** when the binary is built with the `memory-candle` feature.

The design principle: **the agent loop and kernel are unchanged**. `kernel.send_message_streaming()` is unmodified. Voice, text, API, and channel interactions all flow through the same path. The memory layer enriches context silently — the agent receives better recall results without knowing how embeddings were produced.

### Swapping embedding inference and models (config-only)

No rebuild is required to change **model** or **HTTP backend** — edit `~/.openfang/config.toml` and restart the daemon.

| Goal | What to set |
|------|------------------|
| Local Ollama embeddings | `embedding_provider = "ollama"`, `embedding_model = "nomic-embed-text"` (or any model Ollama serves), `[provider_urls]` `ollama = "http://…:11434/v1"` if non-default |
| vLLM / LM Studio / other OpenAI-compatible | `embedding_provider` to the matching name, `embedding_model` to the server’s embedding model id, URL under `provider_urls` |
| In-process Candle (CUDA/CPU) | Build with `cargo build -p openfang-cli --release --features memory-candle`, then `embedding_provider = "candle"` and `embedding_model` = HF id (e.g. `BAAI/bge-small-en-v1.5`) |

**HNSW dimension** must match the model output (e.g. 384 for BGE-small, 768 for nomic-embed-text). The SurrealDB DDL in `openfang-memory` defines the index dimension; change provider/model and DDL together when switching embedding size.

**NER + cross-encoder reranking** use in-process Candle when the binary includes `memory-candle`. They are **not** tied to the embedding provider: set `ner_backend = "candle"` and/or `reranker_backend = "candle"` to run them alongside HTTP embeddings (e.g. Ollama for vectors, Candle on CPU for NER). Omit or `auto` keeps legacy behavior (Candle NER/rerank only when `embedding_provider = "candle"`). Use `none` to force-disable while keeping model ids in the file. A future sidecar or HTTP inference service could add non-Candle backends.

Health detail includes `memory_candle_binary: true|false` so operators know whether NER/rerank can ever be active.

---

## Machine GPU Allocation

| GPU | VRAM | CC | Role | Status |
|-----|------|----|------|--------|
| GTX 970 #0 | 4GB (3.5GB fast) | 5.2 | Available (memory intelligence moved to CPU) | Idle — can be reassigned |
| GTX 1080 #1 | 8GB | 6.1 | Ollama LLMs | Active |
| GTX 1080 #2 | 8GB | 6.1 | Ollama LLMs | Active |
| GTX 970 #3 | 4GB (3.5GB fast) | 5.2 | **Voice inference** — Kokoro / Whisper / LFM2.5-Audio | Active (see `voice-intelligence.md`) |

Memory intelligence runs on **CPU** (i9-9900X with AVX-512). All three Candle BERT models load FP32 in-process with `cuda_device` omitted.

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

Three BERT-family models run in-process via HuggingFace Candle (0.10.0) on the **CPU** (FP32). Models download automatically on first boot to `~/.openfang/models/{model_id}/`. Set `cuda_device = <ordinal>` to use a GPU instead (FP16).

### Phase 1 Models — Active

| Model | Params | RAM (FP32, CPU) | VRAM (FP16, GPU) | Task |
|-------|--------|-----------------|------------------|------|
| `BAAI/bge-small-en-v1.5` | 33M | ~133MB | ~85MB | Sentence embeddings (384d) |
| `dslim/bert-base-NER` | 110M | ~433MB | ~270MB | Named entity extraction |
| `cross-encoder/ms-marco-MiniLM-L-6-v2` | 22M | ~91MB | ~60MB | HNSW candidate reranking |
| **Total** | **165M** | **~657MB** | **~715MB** (+ ~300MB CUDA ctx) | |

Currently running on **CPU (FP32)**. GPU column retained for reference if `cuda_device` is restored.

The kernel holds optional `Arc<>` handles (`ner_driver`, `reranker`) according to `ner_backend` / `reranker_backend` and `MemoryConfig::wants_candle_*`. These are **independent of the embedding provider** — you can use HTTP embeddings with Candle NER/rerank, or all three on Candle, or any mix. When `cuda_device` is set, models load as FP16 on the GPU; when omitted, they load as FP32 on CPU.

Omit `cuda_device` in `[memory]` to force **CPU** inference for Candle (no VRAM; slower, but avoids GPU driver/runtime mismatches).

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

## Data Flows

### Write Path (per conversation turn)

```
Agent Loop (agent_loop.rs)
    │
    ├─ 1. Session write ──────────────────────────► sessions (SurrealDB)
    │
    ├─ 2. Embedding
    │       └─ CandleEmbeddingDriver.embed_one(text)
    │               (spawn_blocking → CPU, FP32)
    │               BertModel forward → mean-pool → L2-norm → Vec<f32> 384d
    │
    ├─ 3. Memory write ───────────────────────────► memories (SurrealDB)
    │       content + embedding → HNSW auto-indexes
    │       content → BM25 auto-indexes
    │
    └─ 4. NER extraction (post-loop, best-effort)
            kernel.populate_knowledge_graph(agent_id, response_text, memory_id)
                CandleNerDriver.extract_entities(text)
                    (spawn_blocking → CPU, FP32)
                    BertModel + token-classification head → BIO tags → entity spans
                        knowledge.add_entity()   → entities (SurrealDB)
                        knowledge.add_relation() → relations (SurrealDB)
            Streaming: spawned as background tokio task after loop returns
            Non-streaming: inline await after loop returns
```

### Read Path (per recall query)

```
Recall Query (agent_loop.rs, top of run_agent_loop / run_agent_loop_streaming)
    │
    ├─ CandleEmbeddingDriver.embed_one(query) → Vec<f32> 384d
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
    └─ Cross-encoder reranking (when reranker loaded)
            CandleReranker.rerank(query, candidates)
                (spawn_blocking → CPU, FP32)
                BertModel (query, doc) pairs → [CLS] → Linear(1) → score
                sorted descending → MemoryFragment[]
```

### Consolidation Path (background, periodic)

```
ConsolidationEngine.run_for_all_agents()
    │
    ├─ SELECT DISTINCT agent_id FROM memories WHERE deleted = false
    │
    └─ Per agent:
            decay_confidence()  confidence × (1 − 0.05)
                WHERE accessed_at < now − 7 days
            prune()  SET deleted = true
                WHERE confidence < 0.1
```

---

## Embedding Latency

| Path | Latency | Notes |
|------|---------|-------|
| Previous: Ollama HTTP | ~887ms | Network + serialization + model load |
| Candle CUDA (healthy) | ~sub-ms–few ms | In-VRAM FP16; first call pays mmap/load |
| Candle CPU (`cuda_device` omitted) | ~5–15ms typical | In-process; depends on CPU (e.g. AVX-512) |
| **No embedding driver** (Candle init failed) | — | Recall uses BM25 / text / recent paths only — **no new vector writes** until a working embedding driver is restored |

Rough order-of-magnitude vs Ollama HTTP for local GPU: **~100–1000×** lower per-call latency when CUDA is working.

BERT weights load at driver init (boot), not lazily on first `remember()` — first `remember()` may still pay tokenization and a cold GPU kernel.

---

## Kernel Integration

```
OpenFangKernel
├─ embedding_driver: Option<Arc<dyn EmbeddingDriver>>
│       CandleEmbeddingDriver — provider="candle", model="BAAI/bge-small-en-v1.5"
│       create_candle_embedding_driver() called at boot (CPU, FP32)
│
├─ ner_driver: Option<Arc<CandleNerDriver>>       [cfg(feature = "memory-candle")]
│       loaded at boot when ner_backend = "candle" (or "auto" + embedding_provider = "candle")
│       passed to populate_knowledge_graph() after each agent loop completes
│
├─ reranker: Option<Arc<CandleReranker>>           [cfg(feature = "memory-candle")]
│       loaded at boot when reranker_backend = "candle" (or "auto" + embedding_provider = "candle")
│       passed into run_agent_loop / run_agent_loop_streaming, reranks recall results
│
└─ populate_knowledge_graph(agent_id, text, memory_id)
        called after agent loop returns (streaming: background spawn; non-streaming: inline)
```

All three Candle handles are `Option` — the system degrades gracefully if a model fails to load.

| Subsystem | If `None` / inactive |
|-----------|----------------------|
| `embedding_driver` | No dense vectors on write; recall skips the HNSW leg (BM25 + structured paths still work). |
| `ner_driver` | No automatic entity/relation extraction after `remember()`. |
| `reranker` | HNSW/BM25 ordering unchanged (no cross-encoder rescoring). |

---

## Health & observability

Authenticated **GET** `/api/health/detail` includes a `memory_intelligence` object:

| Field | Meaning |
|-------|---------|
| `embedding_provider` | Config string, e.g. `candle` |
| `embedding_model` | Configured model id |
| `embedding_active` | `true` only if `embedding_driver` initialized |
| `ner_backend` / `reranker_backend` | Raw config: `auto`, `candle`, `none`, etc. |
| `ner_active` | NER model loaded |
| `reranker_active` | Cross-encoder loaded |
| `cuda_device` | Config value (JSON `null` if unset) |
| `models_cached` | Heuristic: `~/.openfang/models/<embedding_model>/model.safetensors` exists |

Cross-check with `journalctl --user -u openfang` for Candle load warnings (CUDA errors, tokenizer issues).

---

## Active Configuration (`~/.openfang/config.toml`)

```toml
[memory]
embedding_model    = "BAAI/bge-small-en-v1.5"
embedding_provider = "candle"
# cuda_device      = 0                  # Omitted = CPU; integer = CUDA ordinal for GPU
ner_backend        = "candle"           # auto | candle | none
reranker_backend   = "candle"
# ner_model        = "dslim/bert-base-NER"           # default; set null to disable
# reranker_model   = "cross-encoder/ms-marco-MiniLM-L-6-v2"  # default; set null to disable
consolidation_threshold = 10000
decay_rate         = 0.05
```

All three subsystems active on CPU. Verify via `/api/health/detail`:
```json
"memory_intelligence": {
  "embedding_active": true,
  "ner_active": true,
  "reranker_active": true,
  "cuda_device": null
}
```

---

## Phase 2 (Pending)

| Change | What | Blocker |
|--------|------|---------|
| Upgrade embeddings | `nomic-embed-text-v1` (768d) via JinaBert in candle-transformers | HNSW DDL update: 384→768 |
| Upgrade NER | `LiquidAI/LFM2-350M-Extract` ONNX | `protoc` needed for `candle-onnx` |
| Session compaction | `LiquidAI/LFM2-2.6B-Transcript` ONNX Q4 | Same `protoc` requirement |
| SurrealML in-query inference | `ml::` salience scoring + `DEFINE EVENT` auto-tagging | `surrealml-core` ort/ndarray version triangle — needs `surrealdb` 2.7+ |
| Consolidation scheduling | Timer invoking `consolidate()` every N hours | `consolidation_interval_hours` config field exists, no scheduler wired |

---

## Deployment & troubleshooting

**Replacing the daemon binary:** stop the user service before copying over `~/.local/bin/openfang`, otherwise `cp` fails with “Text file busy” (the running process keeps the executable mapped):

```bash
systemctl --user stop openfang
cp target/release/openfang ~/.local/bin/openfang
systemctl --user start openfang
```

**CUDA init failures** (e.g. `CUDA_ERROR_NOT_FOUND`, “named symbol not found” at `BertModel::load`): usually a mismatch between the NVIDIA driver, the CUDA runtime Candle was built against, and/or stale GPU state. Confirm `nvidia-smi`, rebuild after driver upgrades, or temporarily omit `cuda_device` to run Candle on CPU.

**NER tokenizer:** some HF repos (e.g. `dslim/bert-base-NER`) omit `tokenizer.json`. `model_cache::resolve_model` now falls back to downloading `tokenizer.json` from **`bert-base-cased`** when the primary repo returns 404. You can still place a file manually under `~/.openfang/models/<model_id>/tokenizer.json` if needed.

**Cargo / SSH:** workspace `.cargo/config.toml` sets `git-fetch-with-cli = true` so `[patch]` git deps (e.g. `candle-kernels`) fetch via the system `git` + SSH agent. Optional `sccache` + `lld` there speed rebuilds.

---

## Repository State

| Location | Contents |
|----------|----------|
| `surreang` branch (typical) | Memory + Candle integration on prtr |
| `git.ism.la:6666/rtr/candle-kernels` | Forked `candle-kernels` with pre-Volta CC guard |
| `~/.openfang/config.toml` | Live daemon config |
| `~/.openfang/data/openfang.db/` | SurrealKV data (live) |
| `~/.openfang/models/` | HF model cache |

Long-lived feature branches may **diverge from** `origin/main`; merge or cherry-pick upstream fixes (API, security, channels) deliberately rather than blind fast-forward.
