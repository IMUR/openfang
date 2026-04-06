# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
cargo build --workspace              # Full workspace build
cargo build --profile release-fast -p openfang-cli  # Fast release build (thin LTO, 8 CU)
cargo test --workspace               # Run all ~1,744 tests
cargo test -p openfang-kernel        # Single crate tests
cargo test -p openfang-runtime -- test_name  # Single test
cargo clippy --workspace --all-targets -- -D warnings  # Lint (zero warnings enforced)
cargo fmt --all                      # Format (enforced by CI)
cargo run -- doctor                  # Verify local setup
```

CI also runs `cargo audit` (security) and TruffleHog (secrets scan).

## Architecture

OpenFang is an agent operating system — a Cargo workspace of 14 crates with a strict dependency hierarchy:

```
openfang-types          (shared types, no business logic)
    ↓
openfang-memory         (SurrealDB memory substrate, vector search, sessions)
    ↓
openfang-runtime        (agent loop, LLM drivers, 38 tools, WASM sandbox, MCP client)
    ↓
openfang-kernel         (orchestrator: workflows, RBAC, cron, heartbeat, config hot-reload)
    ↓
openfang-api            (Axum 0.8 server: 76 REST/WS/SSE endpoints, OpenAI compat)
    ↓
openfang-cli / openfang-desktop
```

Lateral crates (depended on by kernel): `openfang-channels` (40 adapters), `openfang-wire` (P2P OFP protocol), `openfang-skills` (60 bundled + FangHub marketplace), `openfang-hands` (autonomous capability packages), `openfang-extensions` (MCP templates, credential vault), `openfang-migrate` (OpenClaw importer).

### Key Patterns

- **`KernelHandle` trait** (defined in runtime, implemented on kernel): breaks the circular dependency so runtime tools can call back into the kernel for inter-agent operations.
- **Shared memory**: Fixed UUID `AgentId(Uuid::from_bytes([0..0, 0x01]))` provides cross-agent KV namespace.
- **Daemon detection**: CLI checks `~/.openfang/daemon.json` and pings health endpoint. If running → HTTP mode; otherwise → in-process kernel.
- **Capability-based security**: Every agent operation is checked against granted capabilities before execution.
- **Config**: `KernelConfig` in `openfang-types/src/config.rs` is the single source of truth. All config structs use `#[serde(default)]` for forward-compatible partial TOML.
- **Candle integration** (optional `memory-candle` feature): in-process CUDA inference for embeddings, NER, reranking on local GPU. Patched `candle-kernels` for GTX 970 CC 5.2.

## Code Style

- `rustfmt` with `max_width = 100` (see `rustfmt.toml`)
- `thiserror` for error types; no `unwrap()` in library code — propagate with `?`
- Workspace dependencies in root `Cargo.toml`; justify new deps in PRs
- Use `tempfile::TempDir` for filesystem isolation in tests; random ports for network tests
- Types use `#[serde(default)]` for forward compatibility

## Where Things Live

- Agent templates: `agents/{name}/agent.toml`
- Channel adapters: `crates/openfang-channels/src/{platform}.rs` implementing `ChannelAdapter` trait
- Built-in tools: `crates/openfang-runtime/src/tool_runner.rs` — add impl function, register in `execute_tool` match, add to `builtin_tool_definitions()`
- LLM drivers: `crates/openfang-runtime/src/drivers/`
- API routes: `crates/openfang-api/src/routes.rs`
- Config types: `crates/openfang-types/src/config.rs`
- CLI commands: `crates/openfang-cli/src/main.rs`
