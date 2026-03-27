# Mind — Agent Context

## What This Is

A cognitive architecture for LLMs — persistent identity, autobiographical memory, and epistemic self-awareness. **Pre-implementation stage.** The project is currently documentation and design only.

## Read First

| File | What It Tells You |
|------|-------------------|
| `docs/foundations.md` | Locked-in decisions: Rust toolchain, not-a-RAG principle, naming rules |
| `docs/exploration.md` | Open hypotheses: assertion/inquiry tension, two-track memory, subsystem mappings |
| `docs/research.md` | External influences: DeepSeek Engram, Anthropic sleep-time compute, XTDB/OpenViking patterns |

## Core Concepts

**Two modes:** Assertion (confident default) ↔ Inquiry (triggered by contradiction/correction). Not discrete states — a cognitive tension.

**Two tracks:** Immutable (append-only epistemic history) + Reconciled (living self-model loaded into context).

**Six subsystems:** Trace Store (episodic log), Schema Store (fast KV facts), Similarity Index (vector search), Context Buffer (working memory), Consolidator (background reorganization), Orchestrator (retrieval routing).

## Technical Stack (decided)

- **Language:** Rust
- **Storage:** `redb` + `usearch` + `petgraph`
- **Inference:** Pure Rust — `candle`, `tract`, or `rten` (not Ollama, not Python)
- **Goal:** Single self-contained binary, no external services

### Python & JavaScript (when used)

The implementation is **Rust-first**. These rules apply when you touch Python or JavaScript here (scripts, lint, cluster helpers, ad-hoc tooling)—not for the Mind binary itself.

- **Python:** **uv** is the package manager. Use `uv pip install`, `uv run`, and `uvx` for one-off tools. Do not use `pip`, `pip install`, `python -m venv`, `virtualenv`, or `conda` directly. Virtual environments are an implementation detail managed by uv—do not create or activate them manually.
- **JavaScript:** **bun** is the runtime and package manager. Use `bun install`, `bun run`, and `bunx` for one-off tools. Do not use `npm`, `npx`, `yarn`, or `node` directly unless a specific tool requires it.

## What This Is Not

- **Not Omnibus.** Omnibus (`/mnt/ops/Omnibus`) is a separate ingestion pipeline for normalizing chat archives. Mind is the cognitive project. They share a user but not a codebase.
- **Not a RAG pipeline.** Mind manages memory and epistemic state, not document retrieval.

## How To Work Here

1. **Read `docs/foundations.md` before writing code.** The design decisions there are non-negotiable.
2. **Markdown:** Project rules live in `.markdownlint-cli2.jsonc` at the repo root (vscode-markdownlint and `markdownlint-cli2` both read it). Do not rely on editor-only lint settings for shared behavior.
3. **Keep docs exploratory.** Use open questions, not prescriptive specs. Say "we are exploring X" not "the system does X."
4. **Call things what they are.** If it's a KV store, call it a KV store. No metaphorical naming.
5. **No architecture astronautics.** If you can't test it against real data, don't build it yet.
6. **Cluster context** is in `docs/rtr-cluster.md` if you need to know about the infrastructure.

## Agent skills (when to use)

Skills are loaded from the host’s default skills directory (the same skill packs are mirrored across cluster nodes). Use this section to pick **Mind-appropriate** skills; the full inventory is in `CATALOG.md`.

### Principles

- Prefer skills that match **`docs/foundations.md`**: Rust-first, single binary, `redb` + `usearch` + `petgraph`, pure Rust inference (`candle` / `tract` / `rten`).
- Mind is **not** a traditional RAG pipeline. The orchestrator is currently designed as a memory management layer, though its capabilities may evolve.
- Skills whose descriptions assume **document retrieval RAG**, **LangChain-style agents**, or **hosted vector DBs** can still help for **narrow mechanics** (e.g. embeddings, ANN behavior)—do not let them silently redefine the architecture.

### Design, exploration, and ADRs

- `brainstorming` — Before large design or doc reshaping; keep hypotheses explicit.
- `architecture` — Trade-offs and structuring without over-specifying.
- `architecture-decision-records` / `adr` — Recording locked decisions and supersessions.
- `doc-coauthoring` — Drafting or restructuring design docs and specs.
- `software-architecture` — General quality and boundaries as code appears.
- `writing-plans` / `plan-writing` — Multi-step work with clear checkpoints.
- `mermaid-expert` — Diagrams for subsystems, flows, and two-track memory.

### Rust implementation

- `systems-programming-rust-project` — Bootstrapping and structuring the crate/workspace.
- `rust-pro` — Idiomatic Rust, types, and ecosystem patterns for the binary.
- `rust-async-patterns` — Async I/O, background consolidator-style work, concurrency.
- `memory-safety-patterns` — Ownership, RAII, safe boundaries around hot paths.
- `error-handling-patterns` — `Result`/errors across store and orchestration layers.
- `performance-profiling` — Hot paths: inference, indexing, consolidation loops.

### Append-only trace, projections, and storage modeling

- `event-sourcing-architect` — Conceptual fit for immutable trace + derived views (not mandatory CQRS everywhere).
- `event-store-design` — Event store shape, retention, and projection thinking.
- `database-design` — Schema/index thinking for embedded KV and access patterns.

### Vectors and similarity (Similarity Index)

- `similarity-search-patterns` — ANN/HNSW-style retrieval behavior.
- `vector-index-tuning` — Latency/recall trade-offs for `usearch`.
- `embedding-strategies` — Choosing embedding models and shaping inputs *for the index*, not for document RAG.

### Memory and context (orchestration layer)

- `memory-systems` — Short/long/graph memory *design* analogies; not a fixed product spec.
- `context-manager` / `context-window-management` / `context-degradation` — Context budgeting, routing, and failure modes for what gets loaded into the LLM.
- `conversation-memory` — Session persistence concepts where relevant; Mind is broader than chat logs.

### Heuristic Swarm (orchestrator-coordinated small models)

- `ai-agents-architect`, `autonomous-agents`, `multi-agent-patterns` — Use when designing the **Heuristic Swarm** and orchestrator-coordinated small models; do not let them blindly turn the core memory system into a standard LangChain ReAct loop.

### Inference artifacts

- `hugging-face-cli` — Pulling or caching model weights for Rust-native runtimes (not Python serving stacks).

### Tools and orchestration interfaces

- `tool-design` — Designing interfaces the orchestrator exposes.

### Testing and debugging

- `test-driven-development` / `tdd-workflow` — When tests should drive implementation; keep “test against real data” in mind.
- `unit-testing-test-generate` — Broad unit-test coverage.
- `systematic-debugging` / `debugging-strategies` — Tracing failures across stores and async tasks.
- `sharp-edges` — Risky APIs and configs (persistence, `unsafe`, resource limits).

### Evaluation (later)

- `evaluation` / `llm-evaluation` — Measuring behavior of the LLM-facing layer; frame as **system** evaluation where possible.

### Research and external synthesis

- `investigation-research` — Deep dives for `docs/research.md` and comparable notes.

### Use narrowly (mechanics only; do not import a product architecture wholesale)

- `rag-engineer`, `rag-implementation`, `langchain-architecture`, `langgraph` — Only for specific mechanics (e.g. embedding similarity, patterns worth stealing); do not adopt “RAG app” or LangChain-as-the-system by default.
