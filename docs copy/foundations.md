# Foundations

_The few decisions about Mind that are locked in._

---

## 1. Not a RAG Pipeline

Mind is a cognitive architecture, not a retrieval system.

* **RAG (Retrieval-Augmented Generation):** Retrieves documents to augment prompts and answer queries. The LLM reasons _with_ the retrieved data.
* **Mind:** Maintains an evolving, persistent self-model. The LLM reasons _about_ this model and its own epistemic state. It manages memory; it doesn't just look things up.

## 2. Native Rust Toolchain

The system is built on a self-contained, native Rust stack.

* **Constraint:** Single binary, minimal external C dependencies, no heavy external services.
* **Storage:** `redb` (pure Rust embedded KV) + `usearch` (HNSW vectors) + `petgraph` (associations).
* **Inference (Rust-native):** Pure Rust ML runtimes that can run transformer models (embeddings, NER, classification) directly on GPU — no Ollama, no Python, no HTTP calls.
  * `candle` — Hugging Face's pure Rust ML framework. PyTorch-like API, CUDA/Metal support, direct Hub integration. Best ecosystem for transformer models.
  * `tract` — Pure Rust ONNX/TF inference engine. Zero C deps, tiny footprint, strong WASM support. CPU-focused.
  * `rten` — Pure Rust ONNX runtime with SIMD optimization. Fast model loading, growing transformer support.
* **Why:** To keep the footprint tiny and ensure the architecture is tightly coupled to the orchestration logic, avoiding the bloat and roadmap dependencies of VC-backed or multi-tenant database products. Embedding generation inside the binary means the Similarity Index doesn't depend on an external inference server.
* **Future exploration:** SurrealDB (Rust-native multi-model DB) was evaluated and rejected as a dependency due to BSL 1.1 licensing and multi-tenant bloat. However, its internal architecture—particularly SurrealKV (versioned storage engine) and its HNSW implementation—may be worth studying or extracting patterns from as the storage layer matures.

## 3. Storage Primitives Keep Their Names

Technical infrastructure is called what it is.

* If it's an append-only log, it's called an append-only log. If it's a vector index, it's called a vector index.
* **Why:** Wrapping standard database tables in metaphorical names (e.g., calling a document store a "Biography") creates false expectations and obscures the actual plumbing.

## 4. Cognitive Concepts Live in Orchestration

The cognitive architecture exists in how the primitives are orchestrated, not in the storage layer itself.

* Behaviors like _consolidation_, _assertion_, and _reconsolidation_ are orchestration patterns—how the system moves data, questions it, and updates it—not database wrappers.

## 5. Mind vs. Omnibus Boundary

* **Omnibus:** An ingestion pipeline that normalizes chat archives into a queryable knowledge base for sourcing portfolio/blog content.
* **Mind:** The cognitive project exploring persistent memory and self-awareness for LLMs. They are separate concerns.
