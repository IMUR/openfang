# OpenFang Memory Intelligence Architecture

**Date:** 2026-04-01
**Branch:** `surreang` (prtr local, not pushed to origin)
**Version:** 0.5.1

---

## Overview

The memory layer is a multi-model substrate built on embedded SurrealDB (SurrealKV). Every agent has access to six stores that persist across sessions. Three of those stores are now backed by in-process ML inference running on a dedicated GPU.

The design principle: **the agent loop and kernel are unchanged**. `kernel.send_message_streaming()` is unmodified. Voice, text, API, and channel interactions all flow through the same path. The memory layer enriches context silently — the agent receives better recall results without knowing how they were produced.

---

## Machine GPU Allocation

| GPU | VRAM | CC | Role | Status |
|-----|------|----|------|--------|
| GTX 970 #0 | 4GB (3.5GB fast) | 5.2 | **Memory intelligence** — Candle BERT in-process | Intended; confirm with `/api/health/detail` (`embedding_active`, `ner_active`) — CUDA must load cleanly |
| GTX 1080 #1 | 8GB | 6.1 | Ollama LLMs | Active |
| GTX 1080 #2 | 8GB | 6.1 | Ollama LLMs | Active |
| GTX 970 #3 | 4GB (3.5GB fast) | 5.2 | **Voice inference** — Kokoro / Whisper / LFM2.5-Audio | Pending activation (see `voice-intelligence.md`) |

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

## Candle Inference Backend (GTX 970 #0)

Three BERT-family models run in-process via HuggingFace Candle (0.10.0). Models download automatically on first boot to `~/.openfang/models/{model_id}/`.

### Phase 1 Models — Active

| Model | Params | VRAM (FP16) | Task |
|-------|--------|-------------|------|
| `BAAI/bge-small-en-v1.5` | 33M | ~85MB | Sentence embeddings (384d) |
| `dslim/bert-base-NER` | 110M | ~270MB | Named entity extraction |
| `cross-encoder/ms-marco-MiniLM-L-6-v2` | 22M | ~60MB | HNSW candidate reranking |
| CUDA context | — | ~300MB | Driver overhead |
| **Total** | **165M** | **~715MB** | **2.8GB headroom on GTX 970** |

When CUDA is healthy, all three load as FP16 on the configured device. Inference runs in `tokio::task::spawn_blocking`. The kernel holds optional `Arc<>` handles (`ner_driver`, `reranker`) loaded at boot whenever `embedding_provider = "candle"` (independent of whether the embedding model itself loaded — but a broken CUDA context usually fails all three).

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
Agent Loop
    │
    ├─ 1. Session write ──────────────────────────► sessions (SurrealDB)
    │
    ├─ 2. Embedding
    │       └─ CandleEmbeddingDriver.embed_one(text)
    │               (spawn_blocking → GTX 970 #0, FP16)
    │               BertModel forward → mean-pool → L2-norm → Vec<f32> 384d
    │
    ├─ 3. Memory write ───────────────────────────► memories (SurrealDB)
    │       content + embedding → HNSW auto-indexes
    │       content → BM25 auto-indexes
    │
    └─ 4. NER extraction (background, best-effort)
            CandleNerDriver.extract_entities(text)
                (spawn_blocking → GTX 970 #0, FP16)
                BertModel + token-classification head → BIO tags → entity spans
                    kernel.populate_knowledge_graph()
                        knowledge.add_entity()   → entities (SurrealDB)
                        knowledge.add_relation() → relations (SurrealDB)
```

### Read Path (per recall query)

```
Recall Query
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
│       create_candle_embedding_driver() called at boot
│
├─ ner_driver: Option<Arc<CandleNerDriver>>
│       activate_ner_driver("dslim/bert-base-NER", cuda_device=0)
│
├─ reranker: Option<Arc<CandleReranker>>
│       activate_reranker("cross-encoder/ms-marco-MiniLM-L-6-v2", cuda_device=0)
│
└─ populate_knowledge_graph(agent_id, text, memory_id)
        spawned in background after each remember()
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
| `embedding_model` | Configured HF id |
| `embedding_active` | `true` only if `embedding_driver` initialized |
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
cuda_device        = 0                  # Omit for CPU-only Candle; integer = CUDA ordinal
# ner_model        = "dslim/bert-base-NER"           # default; set null to disable
# reranker_model   = "cross-encoder/ms-marco-MiniLM-L-6-v2"  # default; set null to disable
consolidation_threshold = 10000
decay_rate         = 0.05
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
