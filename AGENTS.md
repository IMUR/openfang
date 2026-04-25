# OpenFang — Agent Context (Reference)

## What This Is

An asynchronous agent runtime, CLI, and daemon for persistent, multi-agent AI interactions. OpenFang acts as the execution kernel, memory substrate, and API backend for conversational LLM agents. Built on a 14-crate Rust workspace, fully async on Tokio, with an embedded SurrealDB substrate and MCP integration.

## Read First


| File                                           | What It Tells You                                          |
| ---------------------------------------------- | ---------------------------------------------------------- |
| `docs/architecture.md`                         | General system architecture and crate topology             |
| `crates/openfang-cli/README.md`                | CLI commands and MCP bindings                              |
| `crates/openfang-api/README.md`                | Axum HTTP daemon routing and web streaming                 |
| `crates/openfang-types/src/config.rs`          | `KernelConfig` doc comments — every config field explained |
| `crates/openfang-runtime/src/drivers/mod.rs`   | `provider_defaults()` — provider wiring and resolution     |
| `crates/openfang-runtime/src/model_catalog.rs` | ~198 built-in model definitions                            |
| `crates/openfang-memory/src/db.rs`             | SurrealDB DDL and substrate setup                          |
| `references/schema-ddl.md`                     | Full SurrealDB schema with field-level docs                |


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
6. **Agent manifest authority:** `~/.openfang/agents/<name>/agent.toml` is the **source of truth** for declarative manifest fields (model, tools, skills, MCP allowlists, temperature, profile, etc.). On boot, the kernel replaces the embedded manifest from disk when the template exists and persists the **normalized** result to SurrealDB as a cache. Workspace corefiles (`SOUL.md`, `AGENTS.md`, `VOICE.md`, `context.md`, …) are **prompt context**, not a parallel control plane. See `crates/openfang-kernel/src/kernel.rs` (`normalize_manifest_for_runtime`, boot restore).
7. **Async memory I/O:** Never `std::mem::drop` an `async` `save_agent` / `delete_session` / `structured_set` future — always `.await` or use the kernel’s `run_memory_sync` bridge from synchronous code. *(Regression coverage: `agent_manifest_authority_test` + prompt bootstrap tests.)*
8. **Use absolute paths in `config.toml`.** Rust does not expand `~`. The systemd unit sets `WorkingDirectory=%h/.openfang`, so a literal `~` creates a nested `~` directory inside `.openfang`.
9. `**workspace/.cargo/config.toml`** sets `git-fetch-with-cli = true` so `[patch]` git dependencies fetch via system `git` and SSH agent. Required for Forgejo-hosted patches.
10. **Standard build for this deployment** (Candle memory pipeline active):
  ```bash
   cargo build --release --features memory-candle -p openfang-cli
  ```

### Configuration authority (operators)

- `**spawn_default_assistant_on_empty_registry**` (default `true`, `KernelConfig`) — disable to boot with an empty registry (no auto `assistant`).
- **LLM auto-detect** on primary driver failure rewrites in-memory `default_model` only; it does not edit `config.toml` on disk (see boot log message).
- **Memory consolidation constants** in `boot_with_config`: `min_confidence` / `decay_age_days` are fixed passed-through values to `MemorySubstrate::open_with_config` until exposed on `MemoryConfig`.
- **Workspace `CONTEXT_FILES`** (`workspace_context.rs`) — list of files scanned for the auto workspace summary; `VOICE.md` and `context.md` are included alongside prompt_builder paths.

## Key Crate Map


| Crate               | Responsibility                                                               |
| ------------------- | ---------------------------------------------------------------------------- |
| `openfang-types`    | Config structs, shared types                                                 |
| `openfang-runtime`  | LLM drivers, model catalog, provider resolution, TTS/STT engines, agent loop |
| `openfang-memory`   | SurrealDB substrate, schema, persistence                                     |
| `openfang-kernel`   | Agent lifecycle coordinator, scheduler, approval gates                       |
| `openfang-api`      | Axum REST + WebSocket daemon, embedded dashboard                             |
| `openfang-cli`      | CLI commands, TUI, MCP stdio server                                          |
| `openfang-channels` | 40 messaging channel adapters (Telegram, Discord, Slack, etc.)               |
| `openfang-wire`     | OFP peer-to-peer agent networking (HMAC-SHA256 auth)                         |
| `openfang-skills`   | 60 bundled skills, FangHub marketplace client                                |
| `openfang-desktop`  | Tauri 2.0 desktop app shell                                                  |
| `openfang-migrate`  | OpenClaw YAML→TOML config migration                                          |


## Streaming Path

The primary real-time interaction path through the system. Any work on the API, TUI, channel bridges, or future input modalities will touch this.

1. Client sends a message (WS text frame, REST POST, or channel adapter)
2. `kernel.send_message_streaming()` (`kernel.rs`) acquires a per-agent lock, loads the `Session` from SurrealDB, and spawns the agent loop
3. The agent loop emits `StreamEvent` variants (`llm_driver.rs:111`) through an `mpsc` channel:
  - `TextDelta` — incremental response text
  - `ThinkingDelta` — reasoning (stripped by `strip_think_tags()` before display)
  - `ToolUseStart` / `ToolInputDelta` / `ToolUseEnd` — tool call lifecycle
  - `ToolExecutionResult` — tool output (emitted by agent loop, not LLM driver)
  - `PhaseChange` — lifecycle indicator (thinking, tool_use, streaming)
  - `ContentComplete` — end of response with `StopReason` and `TokenUsage`
4. The consumer (`ws.rs`, TUI, or channel bridge) reads events from the channel and forwards them to the client, applying debouncing and filtering as appropriate
5. After the stream closes, the kernel writes the session back to SurrealDB (canonical session update, compaction, JSONL mirror) in a background task

**Key files:** `crates/openfang-runtime/src/llm_driver.rs` (StreamEvent enum, LlmDriver trait), `crates/openfang-kernel/src/kernel.rs` (send_message_streaming), `crates/openfang-api/src/ws.rs` (WebSocket consumer with text debouncing)

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

