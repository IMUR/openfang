<p align="center">
  <img src="public/assets/openfang-logo.png" width="160" alt="OpenFang Logo" />
</p>

<h1 align="center">OpenFang — IMUR Fork</h1>
<h3 align="center">What OpenFang could be, with a real memory substrate.</h3>

<p align="center">
  A divergent fork of <a href="https://github.com/RightNow-AI/openfang">RightNow-AI/openfang</a>, rebuilt around SurrealDB&nbsp;3 and an in-process Candle ML stack.<br/>
  <strong>Same binary. Same Hands. New brain.</strong>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/upstream-RightNow--AI%2Fopenfang-lightgrey?style=flat-square" alt="Upstream" />
  <img src="https://img.shields.io/badge/database-SurrealDB%203.0.5-9cf?style=flat-square" alt="SurrealDB 3" />
  <img src="https://img.shields.io/badge/ML-Candle%20(CPU)-orange?style=flat-square" alt="Candle" />
  <img src="https://img.shields.io/badge/voice-operational-brightgreen?style=flat-square" alt="Voice" />
  <img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="MIT" />
</p>

---

## Why this fork exists

Upstream OpenFang ships an excellent agent runtime. Where it stops short is **memory**. Conversations, embeddings, sessions, and tool state live in scattered substrates with layered abstractions ("Layer 1/2/3") that leak into the runtime. Once you start running real agents 24/7, the cracks show: stale embeddings, lossy session boundaries, no inspection surface, no contracts.

This fork answers a single question: **what does OpenFang look like when memory is a first-class substrate, not an afterthought?**

The bet is that the right primitive is a unified, queryable, ML-aware store — and that SurrealDB&nbsp;3 + Candle is enough to build it without leaving the process boundary.

---

## What's different from upstream

### Memory substrate — SurrealDB 3.0.5
Replaces the layered memory split with a single graph+document+vector store. Native datetime, FULLTEXT analyzers, FLEXIBLE objects, and `option<T> | null` enums. Memory operations are now query-able as data instead of opaque API calls.

### Memory dimensions framework
The old "Layer 1/2/3" vocabulary is retired. Memory surfaces are now described along four dimensions: **substrate**, **data model**, **contract**, and **intelligence**. Contracts are enforced — embedding fallbacks preserve classification metadata, sessions roll up into summaries instead of being truncated.

See [`canon/openfang/memory-intelligence.md`](https://git.ism.la/rtr/openfang) for the full framework.

### In-process ML — Candle on CPU
Embeddings, NER, reranking, and classification all run in-process via `candle-rs`. No external embedding API, no GPU required, no subscription. CUDA is compile-time disabled in this deployment; the model loop is CPU-tuned.

| Capability | Model class | Where it runs |
|---|---|---|
| Embeddings | sentence-transformer | in-process, CPU |
| NER | token-classifier | in-process, CPU |
| Reranking | cross-encoder | in-process, CPU |
| Classification | sequence-classifier | in-process, CPU |

Enable with the `memory-candle` feature.

### Memory inspection API
A read-only HTTP surface for introspecting the memory graph: sessions, summaries, embedding coverage, classification metadata. Designed so you can debug an agent's memory the same way you'd debug its code.

### Rolling session summaries
Sessions auto-summarize on close and maintain a rolling summary as they grow. No more lossy truncation when a session crosses the context boundary — the summary is the context.

### Voice pipeline
Operational STT→LLM→TTS pipeline with server-side VAD barge-in. Runs on a single GTX 1080. No Google, no cloud APIs — local models throughout. (Phonemizer quality is a known gap; tracking alternatives.)

### Memory backfill API
Enrich existing memory rows with NER + classification after the fact. Useful when you upgrade a model and want historical memory to benefit.

---

## Status

This is **not a competitor** to upstream OpenFang. It's a research fork exploring a thesis. If the thesis is right, the work belongs upstream; if it's wrong, the fork is the experiment that proved it.

- **171 commits** ahead of `RightNow-AI/main` at last sync
- **93 upstream PRs** curated and merged in
- **~1,744 tests** passing across 14 crates
- Tracks upstream weekly, integrates non-conflicting changes

The canonical development branch lives at [git.ism.la/rtr/openfang](https://git.ism.la/rtr/openfang). This GitHub mirror is curated for visibility.

---

## Quickstart

The binary, agent format, Hands system, and channel adapters are all upstream-compatible. If you already run OpenFang, this is a drop-in replacement with a different memory backend.

```bash
git clone https://github.com/IMUR/openfang
cd openfang
cargo build --profile release-fast -p openfang-cli
./target/release-fast/openfang doctor   # verify setup
./target/release-fast/openfang start
```

To enable in-process ML:

```bash
cargo build --profile release-fast -p openfang-cli --features memory-candle
```

For the upstream binary install path and the full Hands documentation, see [openfang.sh/docs](https://openfang.sh/docs) — most of it applies here unchanged.

---

## What's not here yet

Honest list:
- **No published binaries.** Build from source.
- **Phonemizer quality** is mediocre (espeak-ng); replacement under evaluation.
- **No hosted dashboard.** Run locally.
- **Breaking changes** between minor versions. Pin to a commit.
- **CUDA disabled** at compile time in this deployment. The code paths exist; the build doesn't enable them.

---

## Contributing

The fork accepts PRs that align with the memory-substrate thesis. For upstream-shaped changes, please open them at [RightNow-AI/openfang](https://github.com/RightNow-AI/openfang) — they'll reach this fork through the regular sync.

Conventions:
- `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings` clean
- No `unwrap()` in library code
- New deps justified in the PR

---

## Credit

OpenFang is the work of the [RightNow-AI](https://github.com/RightNow-AI) team. This fork is grateful for that foundation and exists only because it's worth building on.

---

<p align="center">
  <a href="https://github.com/RightNow-AI/openfang">Upstream</a> &bull;
  <a href="https://git.ism.la/rtr/openfang">Canonical mirror</a> &bull;
  <a href="https://openfang.sh/docs">Upstream docs</a>
</p>
