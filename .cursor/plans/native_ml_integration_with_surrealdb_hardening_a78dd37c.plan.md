---
name: Native ML Integration with SurrealDB Hardening
overview: "Two-phase approach: (1) Immediate SurrealDB hardening with P0 blocking I/O fix and schema DDL, (2) Add openfang-embedding crate with Burn+MiniLM or Candle+LFM2 for local inference, embedding dimension locked to 384 for HNSW index."
todos:
  - id: add-schema-ddl
    content: Add DEFINE TABLE/INDEX/ANALYZER DDL to db::init() with DIMENSION 384 hardcoded
    status: pending
  - id: rename-sqlite-path
    content: Rename sqlite_path to db_path in config.rs and kernel.rs
    status: pending
  - id: update-readme
    content: Update README.md feature table from SQLite + vector to SurrealDB (multi-model)
    status: pending
  - id: create-embedding-crate
    content: Create openfang-embedding crate with Burn+MiniLM or Candle+LFM2 backend
    status: pending
  - id: wire-consolidation
    content: Wire consolidate() to iterate all agents and call decay/prune/purge
    status: pending
  - id: multi-hop-graph
    content: Implement max_depth in knowledge.rs query_graph for multi-hop traversal
    status: pending
  - id: native-hnsw-search
    content: Replace brute-force cosine re-ranking with SurrealDB <|K,EF|> operator
    status: pending
  - id: agent-serialization
    content: Refactor agent storage from double-serialized JSON to native SurrealDB document fields
    status: pending
isProject: false
---

## Phase 1: Immediate Fixes (no model decision required)

These can ship now regardless of which ML backend you choose:

### P0: Blocking I/O Fix (DONE)

- `write_jsonl_mirror()` now wraps filesystem calls in `tokio::task::spawn_blocking`
- Files: `crates/openfang-memory/src/session.rs`, `crates/openfang-memory/src/substrate.rs`, `crates/openfang-kernel/src/kernel.rs`

### P0: Schema DDL with Hardcoded Dimension

- Add `DEFINE TABLE/INDEX/ANALYZER` statements to `db::init()`
- **Key decision**: Hardcode `DIMENSION 384` in HNSW index
- This locks in MiniLM (Burn) or LFM2-ColBERT-350M (Candle) as the first supported embedding model
- Future models would need migration scripts to change index dimension
- File: `crates/openfang-memory/src/db.rs`

### P1: Cosmetics

- Rename `sqlite_path` to `db_path` in `crates/openfang-types/src/config.rs:1500` and `crates/openfang-kernel/src/kernel.rs:561`
- Update README.md feature table from "SQLite + vector" to "SurrealDB (multi-model)"

---

## Phase 2: Native ML Integration (requires model decision)

### New Crate: `openfang-embedding`

Two backend options:

**Option A: Burn + minilm-burn**

- Pure Rust, no unsafe, backend-agnostic (CUDA/Metal/WGPU/CPU)
- Fixed 384-dim output
- Apache-2.0 / MIT dual license
- `tracel-ai/models` repo has ready-made implementation

**Option B: Candle + LFM2-ColBERT-350M**

- ONNX runtime, Liquid AI's own Candle fork with Metal optimizations
- 350M parameters, designed for retrieval embeddings
- Would also unlock path to other Liquid models:
  - LFM2-350M-Extract (structured extraction)
  - LFM2-1.2B-RAG (local RAG)
  - LFM2.5-Audio-1.5B (STT/TTS)
  - LFM2-2.6B-Transcript (session compaction)
  - LFM2.5-1.2B-Instruct (local LLM fallback)

### Architecture

```
subgraph embedding [Embedding Backend]
    subgraph burn [Burn + MiniLM]
        subgraph candle [Candle + LFM2]
    end
end
    
    MemorySubstrate -- stores embeddings
    SemanticStore -- queries embeddings
    Kernel -- selects backend at boot
end
```

### Config Extension

```toml
[embedding]
# backend = "burn" | "candle" | "external" (default: burn)
backend = "burn"
# model = "minilm-l6" | "lfm2-colbert" (default: minilm-l6)
model = "minilm-l6"
# Dimension is FIXED at 384 for local backends
# External backends (openai, ollama) are still supported but become optional overrides
```

### Boot Sequence Change

1. Config loaded
2. **Embedding backend initialized** (Burn or Candle)
3. Dimension is now **known at compile time** (384)
4. `MemorySubstrate::open()` calls `db::init()` with DDL including `DIMENSION 384`
5. Kernel starts

---

## Phase 3: Future Expansion (after ML integration lands)

### P1: Wire `consolidate()`

- Iterate all agents, call decay/prune/purge on ConsolidationEngine

### P1: Multi-hop graph traversal

- Implement `max_depth` in `knowledge.rs` query_graph

### P2: Native HNSW vector search

- Replace brute-force cosine re-ranking with SurrealDB `<|K,EF|>` operator

### P2: Agent serialization refactor

- Store agent fields natively in SurrealDB instead of double-serialized JSON

### Future: Full Liquid Stack

- LFM2-1.2B-RAG for local retrieval-augmented reasoning
- LFM2.5-Audio-1.5B to replace Whisper + Kokoro
- LFM2-2.6B-Transcript for session compaction
- LFM2.5-1.2B-Instruct as local LLM fallback

