# OpenFang — Agent Context (Reference)

## What This Is

An asynchronous agent runtime, CLI, and daemon for persistent, multi-agent AI interactions. OpenFang acts as the execution kernel, memory substrate, and API backend for conversational LLM agents. Built on a 14-crate Rust workspace, fully async on Tokio, with an embedded SurrealDB substrate and MCP integration.

## Read First

| File | What It Tells You |
|------|-------------------|
| `docs/architecture.md` | General system architecture and crate topology |
| `crates/openfang-cli/README.md` | CLI commands and MCP bindings |
| `crates/openfang-api/README.md` | Axum HTTP daemon routing and web streaming |
| `crates/openfang-types/src/config.rs` | `KernelConfig` doc comments — every config field explained |
| `crates/openfang-runtime/src/drivers/mod.rs` | `provider_defaults()` — provider wiring and resolution |
| `crates/openfang-runtime/src/model_catalog.rs` | ~198 built-in model definitions |
| `crates/openfang-memory/src/db.rs` | SurrealDB DDL and substrate setup |
| `references/schema-ddl.md` | Full SurrealDB schema with field-level docs |

## Core Concepts

**Asynchronous First:** The entire system — from Axum API boundaries down to the embedded SurrealDB substrate — runs asynchronously on Tokio. Zero `spawn_blocking` calls in the hot path.

**Substrate Memory:** All agent memory, knowledge graphing, and session states are powered by an embedded SurrealDB process. Legacy SQLite code has been removed entirely.

**Decoupled Architecture:** `openfang-kernel` drives core agent lifecycles, `openfang-memory` handles persistence, `openfang-api` exposes REST and WebSocket interfaces, and `openfang-cli` drives desktop and TUI interactions.

**MCP Integration:** OpenFang runs as an MCP (Model Context Protocol) server over stdio, bridging hosted agents as runnable JSON-RPC tools for external aggregators.

## Technical Stack

- **Language:** Rust (idiomatic, async)
- **Runtime:** Tokio
- **API Framework:** Axum
- **Storage:** SurrealDB (embedded key-value and graph)
- **Distribution:** Self-contained native binary (`openfang`)

### Python & JavaScript (when used)

The core is **Rust-first**. These rules apply to one-off scripts (linters, testers, scrapers).

- **Python:** Use **uv** as the package manager (`uv pip install`, `uv run`). Do not use `pip` or virtual environments manually.
- **JavaScript:** Use **bun** as the runtime/manager (`bun run`, `bunx`). Do not use `npm` unless requested.

## How To Work Here

1. **Keep it Async.** Never fall back to `std::sync` primitives or blocking database connections. If a future blocks, use `tokio::sync` equivalents or `rt.block_on()` cleanly if bounded from a truly synchronous origin (like `main()`).
2. **SurrealDB Over SQLite.** Do not resurrect `rusqlite` code or patterns. Use SurrealDB queries, edges, and graphs exclusively.
3. **Markdown:** Project rules live in `.markdownlint-cli2.jsonc` at the repo root.
4. **Tool the Tooling:** When building capabilities or bindings, adhere aggressively to the MCP specification.
5. **Config Semantics:** `KernelConfig` uses flat maps (`provider_urls`, `provider_api_keys`), not nested provider tables. Nested `[providers.X]` blocks are silently ignored.

## Key Crate Map

| Crate | Responsibility |
|-------|---------------|
| `openfang-types` | Config structs, shared types |
| `openfang-runtime` | LLM drivers, model catalog, provider resolution |
| `openfang-memory` | SurrealDB substrate, schema, persistence |
| `openfang-kernel` | Agent lifecycle coordinator |
| `openfang-api` | Axum REST + WebSocket daemon |
| `openfang-cli` | CLI commands, TUI, MCP stdio server |

## Agent Skills (when to use)

Skills are loaded from the host's default skills directory. Use this section to pick **OpenFang-appropriate** skills.

### Rust Implementation & Architecture

- `systems-programming-rust-project` — Bootstrapping and structuring the workspace/crates.
- `rust-pro` — Idiomatic Rust, advanced type gymnastics, and trait implementations.
- `rust-async-patterns` — Critical for Tokio runtimes, Axum state, traits in futures, and avoiding closure issues in streaming API handlers.
- `backend-architect` — Macro-level API and daemon structuring.

### Storage & Substrate

- `database-optimizer` — SurrealDB query/graph optimization.
- `database-design` — Designing schemas for the `openfang-memory` layer.
- `event-sourcing-architect` — Conceptual patterns for immutable trace and derived views.

### Agent Orchestration & MCP

- `ai-agents-architect` / `autonomous-agents` — When adjusting how agents interact, process tools, or handle loop context.
- `mcp-builder` — Enhancing the Model Context Protocol bindings.
- `multi-agent-patterns` — Multi-agent coordination and communication.
- `tool-design` — Designing interfaces the agent orchestrator exposes.

### API & Networking

- `api-design-principles` — REST endpoint design and versioning.
- `api-patterns` — API style decisions (REST, streaming, WebSocket).
- `error-handling-patterns` — `Result`/errors across API boundaries and substrate layers.

### Testing and Debugging

- `systematic-debugging` / `debugging-strategies` — Tracing `tokio` thread panics or async closure failures.
- `unit-testing-test-generate` — Keeping coverage high, particularly around TUI and MCP responses.
- `test-fixing` — Unbreaking the build.
- `debugging-strategies` — Cross-crate failure tracing.

### Refactoring & Safety

- `code-refactoring-refactor-clean` — When transitioning APIs or refactoring trait implementations.
- `memory-safety-patterns` — Ownership, RAII, safe boundaries around async hot paths.

### Design and Planning

- `brainstorming` — Before large design changes.
- `architecture` / `software-architecture` — Trade-offs and crate boundary decisions.
- `writing-plans` / `plan-writing` — Multi-step work with clear checkpoints.
- `mermaid-expert` — Diagrams for architecture, flows, and substrate topology.
