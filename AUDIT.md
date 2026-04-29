# OpenFang Runtime Audit Reference

**Audit completed:** 2026-04-22  
**Audit scope:** `openfang-types`, `openfang-runtime`, `openfang-skills`, `openfang-api`, `openfang-cli`  
**Final state:** `cargo test --workspace --exclude openfang-desktop` → exit 0

---

## Three-Axis Triage Framework

Use this to triage any future finding before deciding how to fix it.

| Axis | Question | Categories |
|------|----------|------------|
| **Internal history** | What happened to this codebase over time? | Migration residue · Aspirational config · Backward-compat shim |
| **Internal sync** | How do crates fall out of agreement? | Cross-crate drift · Async hygiene |
| **External reality** | How did the outside world shift under us? | Platform decay |

**Migration residue** — survived a system swap because nothing forced removal.  
**Aspirational config** — written for a system that was never implemented.  
**Backward-compat shim** — deliberate coexistence of old and new; do not remove without confirming all callers are gone.  
**Cross-crate drift** — one crate's API changed; callers in other crates weren't updated atomically.  
**Async hygiene** — blocking I/O or subprocess calls inside async context on a Tokio thread.  
**Platform decay** — an external endpoint, model ID, or API auth scheme changed and the code didn't follow.

---

## What Was Audited and Fixed

### `openfang-types/src/config.rs`

| Finding | Category | Fix applied |
|---------|----------|-------------|
| `stt_model` default `"parakeet-tdt-0.6b"` | Migration residue | → `"parakeet-tdt-0.6b-v2"` |
| `tts_voice` default `"chatterbox"` | Migration residue | → `"default"` (Chatterbox uses this identifier) |
| `listen_addresses` default used multiaddr format | Aspirational config | → `"0.0.0.0:4478"` (plain TCP) |
| `sqlite_path` field still present | Backward-compat shim | Retained with `#[deprecated]` doc comment |
| Rusqlite TODO comment | Tech-debt commentary | Verified accurate, left intact |
| `adapter_count` default 40 | Migration residue | Updated to match actual adapter count |
| `TtsConfig`/`TtsEngine` structs | Incorrectly flagged as dead | Verified live; TTS cloud tool ≠ voice pipeline |
| ElevenLabs model `eleven_monolingual_v1` | Platform decay | → `eleven_turbo_v2_5` |

### `openfang-types/src/voice.rs`

| Finding | Category | Fix applied |
|---------|----------|-------------|
| `stt_engine` default referred to old engine name | Migration residue | Updated to concrete Parakeet TDT name |
| Doc comments used generic "any STT" language | Migration residue | Updated to describe actual pipeline |

### `openfang-runtime/src/web_search.rs`

| Finding | Category | Fix applied |
|---------|----------|-------------|
| Tavily API key sent in JSON body | Platform decay | → `Authorization: Bearer` header |

### `openfang-runtime/src/tts.rs`

| Finding | Category | Fix applied |
|---------|----------|-------------|
| Test assertion checked for `eleven_monolingual_v1` | Cross-crate drift | Updated to `eleven_turbo_v2_5` |

### `openfang-runtime/src/web_fetch.rs`

| Finding | Category | Fix applied |
|---------|----------|-------------|
| `"metadata.aws.internal"` in SSRF blocklist | Aspirational config | Replaced with explanatory comment (AWS IMDS has no named hostname) |

### `openfang-runtime/src/browser.rs`

| Finding | Category | Fix applied |
|---------|----------|-------------|
| Chrome UA version `131` | Platform decay | → `136`; maintenance comment added |
| `std::fs::create_dir_all` / `std::fs::write` in async screenshot handler | Async hygiene | → `tokio::fs` equivalents |
| `std::process::Command::new("which")` in Chromium discovery | Async hygiene | → pure Rust PATH walk via `std::env::split_paths` |

### `openfang-runtime/src/host_functions.rs`

| Finding | Category | Fix applied |
|---------|----------|-------------|
| `"metadata.aws.internal"` in SSRF blocklist | Aspirational config | Replaced with explanatory comment |

### `openfang-runtime/src/embedding.rs`

| Finding | Category | Fix applied |
|---------|----------|-------------|
| `embedding_to_bytes` doc comment said "SQLite BLOB storage" | Migration residue | Updated to describe actual use |

### `openfang-runtime/src/mcp_server.rs`

| Finding | Category | Non-finding / fix |
|---------|----------|-------------|
| `PROTOCOL_VERSION: "2024-11-05"` | Backward-compat shim | No change — stdio clients pre-date 2025-03-26; doc comment added to explain |

### `openfang-skills/src/marketplace.rs`

| Finding | Category | Fix applied |
|---------|----------|-------------|
| Entire module | Cross-crate drift | Module deleted; callers in `openfang-api` and `openfang-cli` rewired to `clawhub::ClawHubClient` |

### `openfang-api/src/routes.rs` and `openfang-cli/src/main.rs`

| Finding | Category | Fix applied |
|---------|----------|-------------|
| `install_skill`, `marketplace_search`, `cmd_skill_install`, `cmd_skill_search` called deleted `marketplace` module | Cross-crate drift | Rewired to `openfang_skills::clawhub::ClawHubClient` |

### Non-findings (verified clean, no action needed)

- `copilot_oauth.rs` — GitHub OAuth endpoints, client ID, RFC 8628 error handling: all current
- `mcp.rs` — both transport variants (SSE/HTTP), rmcp SDK usage, SSRF check: all correct
- `image_gen.rs` — DALL-E endpoint and API format: current
- `web_content.rs` — HTML parsing, `find_ci()` Unicode safety, content boundary generation: correct
- `llm_driver.rs` — Anthropic streaming events, tool use lifecycle, thinking/strip logic: current
- Perplexity `"sonar"` model ID: current production model
- ElevenLabs `eleven_multilingual_v2` alternative reference: valid
- MCP `tools/list`, `tools/call`, JSON-RPC 2.0 error codes: all correct
- `host_functions.rs` path traversal and capability gates: correct
- `embedding.rs` cosine similarity, batch embed, dimension inference: correct
- `browser.rs` CDP connection, `--headless=new` flag, `/json/list` endpoint: all current

---

## Re-Check Schedule

Different surfaces decay at different rates. This schedule tells you when to re-verify each surface without doing a full audit every time.

### Every 6 months (fast-moving external surfaces)

| Surface | What to check | Why |
|---------|--------------|-----|
| `browser.rs:254` Chrome UA version | Current Chrome stable version (update if >2 major behind) | Chrome releases every 4 weeks |
| `tts.rs` ElevenLabs model defaults | [ElevenLabs model registry](https://elevenlabs.io/docs/api-reference/models) | Model registry turns over annually |
| `web_search.rs` Tavily auth | Tavily changelog for auth scheme changes | API v2 migration completed; v3 possible |
| `mcp.rs` / `mcp_server.rs` | MCP spec changelog at modelcontextprotocol.io | Spec still evolving; check for new transport types |

### Annually (stable but not immortal)

| Surface | What to check | Why |
|---------|--------------|-----|
| `copilot_oauth.rs` client ID | GitHub Copilot developer docs | Public client IDs are stable but not guaranteed |
| `copilot_oauth.rs` token URL | Same | GitHub occasionally deprecates old OAuth endpoints |
| `embedding.rs` `infer_dimensions()` table | Model provider docs for new embeddings | New model families appear annually |
| `model_catalog.rs` provider base URLs | Each provider's API reference | Base URLs are stable but providers occasionally rename paths |

### On upstream merge (cross-crate sync)

Any time you pull from upstream, grep for new references to `marketplace`, `fanghub`, `FangHub`, `libp2p`, `rusqlite` across all crates. If upstream reintroduces them, evaluate whether to accept or reject the merge hunk.

```bash
# Run after every upstream merge
git grep -n "marketplace\|fanghub\|FangHub\|libp2p\|rusqlite" -- '*.rs'
```

### After adding a new tool handler

Before merging any new tool that makes HTTP calls:
1. Check auth scheme against provider's current docs (not last year's SDK example)
2. Verify the endpoint path hasn't been versioned away
3. Confirm the model/engine ID is in the current provider catalog

---

## Known Intentional Patterns (Don't "Fix" These)

| Code | Pattern | Reason to leave alone |
|------|---------|----------------------|
| `sqlite_path` field in `KernelConfig` | Backward-compat shim | Accepts old config files without crashing |
| `LangChain`/`AutoGpt` in `MigrateSource` | Backward-compat shim | Public API; removing breaks external match arms and upstream re-introduces them |
| `CronJob::delivery` field | Backward-compat shim | Old config files use this field name |
| `mcp_server.rs` `PROTOCOL_VERSION: "2024-11-05"` | Backward-compat shim | Stdio clients (Claude Desktop, VS Code) pre-date 2025-03-26 |
| `ClawHubListResponse` / `ClawHubSearchResults` type aliases | Backward-compat shim | `#[deprecated]`; will be removed after callers are updated |
| `std::fs` in `host_functions.rs` | Intentional | `host_functions.rs` is called from WASM sandbox, which is sync. No Tokio context available |
| `state.tokio_handle.block_on(...)` in `host_functions.rs` | Intentional | The WASM host bridge is sync by design; `block_on` is the correct bridge pattern |

---

## Environment Notes

- `openfang-desktop` requires `pango` system headers (GTK). Excluded from `cargo test` with `--exclude openfang-desktop` on hosts without GTK dev packages installed.
- `TAVILY_API_KEY`, `ELEVENLABS_API_KEY`, and provider-specific keys must be in the environment for integration tests. Unit tests mock these paths.
