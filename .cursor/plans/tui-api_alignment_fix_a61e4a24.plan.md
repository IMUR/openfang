---
name: TUI-API Alignment Fix
overview: Comprehensive fix for the TUI data layer in `crates/openfang-cli/src/tui/` to align all daemon JSON parsing with actual API response shapes, correct field name mismatches, and add the missing `/approvals`, `/approve`, `/reject` slash commands.
todos:
  - id: phase1-envelopes
    content: Fix all 12 JSON envelope mismatches in event.rs (body.as_array -> body["key"].as_array for models, tools, channels, sessions, skills, peers, audit, logs, dashboard-audit, mcp_servers, security, memory_kv)
    status: completed
  - id: phase2-fields
    content: Fix all field name mismatches in event.rs (dashboard uptime/provider/model, models cost fields, workflows created, triggers fires, sessions id/created, audit agent, peers agent_count, usage by-model/by-agent, test_provider, create_workflow/trigger)
    status: completed
  - id: phase3-approvals-mod
    content: Add /approvals, /approve, /reject slash commands to handle_slash_command in mod.rs with daemon+in-process support, update /help text
    status: completed
  - id: phase3-approvals-chat
    content: Add /approvals, /approve, /reject, /agents slash commands to handle_slash_command in chat_runner.rs with daemon+in-process support, update /help text
    status: completed
  - id: build-verify
    content: Full release build, deploy binary, verify TUI Settings models/tools, dashboard fields, and approval commands work
    status: pending
isProject: false
---

# TUI-API Alignment: Complete Data Layer Fix

## Root Cause

The API response envelopes were standardized (bare arrays wrapped in `{"key": [...], "total": N}`) and field names evolved, but the TUI's `event.rs` fetchers were never recompiled against the updated shapes. Additionally, approval slash commands exist in the channel bridge and CLI but were never wired into the TUI.

## Scope

All changes are in **one crate**: `crates/openfang-cli/src/tui/`. Three files:

- [event.rs](crates/openfang-cli/src/tui/event.rs) — 17 broken daemon fetchers
- [mod.rs](crates/openfang-cli/src/tui/mod.rs) — missing slash commands + help text
- [chat_runner.rs](crates/openfang-cli/src/tui/chat_runner.rs) — same missing slash commands

No API changes. No type changes. No new dependencies.

---

## Phase 1: Fix JSON Envelope Mismatches (event.rs)

These functions call `body.as_array()` but the API returns `{"key": [...]}`:

- **spawn_fetch_models** (line 1859): `body.as_array()` -> `body["models"].as_array()`
- **spawn_fetch_tools** (line 1891): `body.as_array()` -> `body["tools"].as_array()`
- **spawn_fetch_channels** (line 587): `body.as_array()` -> `body["channels"].as_array()`
- **spawn_fetch_sessions** (line 1230): `body.as_array()` -> `body["sessions"].as_array()`
- **spawn_fetch_skills** (line 1417): `body.as_array()` -> `body["skills"].as_array()`
- **spawn_fetch_peers** (line 2035): `body.as_array()` -> `body["peers"].as_array()`
- **spawn_fetch_audit** (line 1709): `body.as_array()` -> `body["entries"].as_array()`
- **spawn_fetch_logs** (line 2070): `body.as_array()` -> `body["entries"].as_array()`

Special cases (not simple envelope swaps):

- **spawn_fetch_dashboard** audit part (line 552): `body.as_array()` -> `body["entries"].as_array()`
- **spawn_fetch_mcp_servers** (line 1572): API returns `{configured: [...], connected: [...]}` — needs restructure to merge both arrays
- **spawn_fetch_security** (line 1633): API returns nested object `{core_protections, configurable, monitoring}` — needs restructure to flatten into feature list
- **spawn_fetch_memory_kv** (line 1325): API returns `{"kv_pairs": [{key, value}]}` — needs parse rewrite

## Phase 2: Fix Field Name Mismatches (event.rs)

- **spawn_fetch_dashboard** (line 531): `uptime_secs` -> `uptime_seconds`, `provider` -> `default_provider`, `model` -> `default_model`
- **spawn_fetch_dashboard** audit: `agent` -> `agent_id`
- **spawn_fetch_models** (line 1859): `cost_input` -> `input_cost_per_m`, `cost_output` -> `output_cost_per_m`
- **spawn_fetch_workflows** (line 688): `created` -> `created_at`
- **spawn_fetch_workflow_runs** (line 723): `duration`/`output` -> `steps_completed`/`started_at`/`completed_at`
- **spawn_fetch_triggers** (line 846): `fires` -> `fire_count`
- **spawn_fetch_sessions** (line 1230): `id` -> `session_id`, `created` -> `created_at`, `agent_name` may need fallback
- **spawn_fetch_audit** (line 1709): `agent` -> `agent_id`, `tip_hash` -> top-level not per-row
- **spawn_fetch_logs** (line 2070): `agent` -> `agent_id`
- **spawn_fetch_peers** (line 2035): `agent_count` -> derive from `agents` array length
- **spawn_fetch_usage** by-model (line 1743): `model_id` -> `model`, `calls` -> `call_count`, envelope `{"models": [...]}`
- **spawn_fetch_usage** by-agent: `agent_name` -> `name`, envelope `{"agents": [...]}`
- **spawn_test_provider** (line 1982): reads `message` but API returns `status`/`latency_ms`/`error`
- **spawn_create_workflow** (~804): reads `body["id"]` but API returns `workflow_id`
- **spawn_create_trigger** (~881): reads `body["id"]` but API returns `trigger_id`

## Phase 3: Add Approval Slash Commands

### mod.rs (full TUI)

Add three match arms in `handle_slash_command()` before the `_ =>` catch-all (~line 2182):

- `/approvals` — list pending via `GET /api/approvals` (daemon) or `kernel.approval_manager.list_pending()` (in-process)
- `/approve <id>` — resolve via `POST /api/approvals/{id}/approve` (daemon) or `kernel.approval_manager.resolve()` (in-process)
- `/reject <id>` — resolve via `POST /api/approvals/{id}/reject` (daemon) or `kernel.approval_manager.resolve()` (in-process)

Update `/help` text (~line 2006) to include the three new commands.

### chat_runner.rs (standalone chat)

Same three match arms in `handle_slash_command()` (~~line 368).
Update `/help` text (~~line 272).
Also add `/agents` which exists in the full TUI but is missing here.

---

## What We Are NOT Changing

- **InProcess stubs**: Most `BackendRef::InProcess(_)` branches return empty data. These are lower priority since the deployment runs daemon mode. Fixing them all would double the changeset for functionality that isn't being used right now. The models InProcess branch (which we know about) will be fixed since it's trivial, but the rest stay as-is.
- **API endpoints**: No changes to `routes.rs` or any API handler.
- **Type definitions**: The `SettingsState`, `AuditRow`, etc. structs in `event.rs` and screen files may need minor field additions/renames if the API provides more data now, but we will keep structural changes minimal — only where a field name rename is needed to avoid confusion.

## Build and Verify

- `cargo build --release` from `/home/prtr/prj/openfang`
- Stop daemon, replace binary, restart
- Verify: TUI Settings > Models tab shows the full catalog
- Verify: TUI Settings > Tools tab shows built-in + MCP tools
- Verify: `/approvals` in chat shows "No pending approvals" or any pending list
- Verify: Dashboard shows correct uptime, provider, model

