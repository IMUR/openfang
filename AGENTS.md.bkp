# OpenFang — Agent Context

## What This Is

An asynchronous agent runtime, CLI, and daemon for persistent, multi-agent AI interactions. OpenFang acts as the execution kernel, memory substrate, and API backend for conversational LLM agents.

## Read First

| File | What It Tells You |
|------|-------------------|
| `docs/architecture.md` | General system architecture. |
| `crates/openfang-cli/README.md`| Details about the CLI and MCP bindings. |
| `crates/openfang-api/README.md`| Axum HTTP daemon routing and web streaming concepts. |

## Core Concepts

**Asynchronous First:** The entire system—from Axum API boundaries down to the embedded SurrealDB substrate—runs asynchronously on Tokio. There are zero blocking calls (`spawn_blocking`) tolerated in the hot path.

**Substrate Memory:** All agent memory, knowledge graphing, and session states are powered by an embedded SurrealDB process. Legacy SQLite code has been removed.

**Decoupled Architecture:** `openfang-kernel` drives core agent lifecycles, `openfang-memory` handles persistence, `openfang-api` exposes REST and WebSocket interfaces, and `openfang-cli` drives desktop and TUI interactions.

**MCP Integration:** OpenFang runs seamlessly as an MCP (Model Context Protocol) server over stdio, bridging its hosted agents out as runnable JSON-RPC tools for external aggregators.

## Technical Stack (decided)

- **Language:** Rust (Idiomatic, Async)
- **Runtime:** Tokio
- **API Framework:** Axum
- **Storage:** SurrealDB (Embedded Key-Value and Graph)
- **Distribution:** Self-contained native binary (`openfang`)

### Python & JavaScript (when used)

The core is **Rust-first**. These rules apply when touching one-off Python or JavaScript scripts (linters, testers, scrapers).

- **Python:** Use **uv** as the package manager (`uv pip install`, `uv run`). Do not use `pip` or virtual environments manually.
- **JavaScript:** Use **bun** as the runtime/manager (`bun run`, `bunx`). Do not use `npm` unless requested.

## How To Work Here

1. **Keep it Async.** Never fallback to `std::sync` primitives or blocking Database connections. If a future blocks, use `tokio::sync` equivalents or `rt.block_on()` cleanly if bounded from a truly synchronous origin (like `main()`).
2. **SurrealDB Over SQLite.** We are entirely migrated. Do not resurrect `rusqlite` code or patterns. Use SurrealDB queries, edges, and graphs.
3. **Markdown:** Project rules live in `.markdownlint-cli2.jsonc` at the repo root.
4. **Tool the Tooling:** When building capabilities or bindings, adhere aggressively to the MCP specification (`mcp.rs`).

## Agent skills (when to use)

Skills are loaded from the host’s default skills directory. Use this section to pick **OpenFang-appropriate** skills.

### Rust Implementation & Architecture
- `systems-programming-rust-project` — Bootstrapping and structuring the workspace/crates.
- `rust-pro` — Idiomatic Rust, advanced type gymnastics, and trait implementations.
- `rust-async-patterns` — Critical for Tokio runtimes, Axum state, traits in futures, and avoiding closures issues in streaming API handlers.
- `backend-architect` — Good for macro-level API and daemon structuring.

### Storage & Substrate
- `database-optimizer` — SurrealDB query/graph optimization.
- `database-design` — Designing schemas for the `openfang-memory` layer.

### Agent Orchestration & MCP
- `ai-agents-architect` / `autonomous-agents` — When adjusting how agents interact, process tools, or handle loop context.
- `mcp-builder` — Enhancing the Model Context Protocol bindings.

### Testing and Debugging
- `systematic-debugging` / `debugging-strategies` — Tracing `tokio` thread panic or async closure bounds failures.
- `unit-testing-test-generate` — Keeping coverage high, particularly around TUI and MCP responses.
- `test-fixing` — Unbreaking the build.

### Refactoring & Safety
- `code-refactoring-refactor-clean` — When transitioning APIs or refactoring trait implementations.
