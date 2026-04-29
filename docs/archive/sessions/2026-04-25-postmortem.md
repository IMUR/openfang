# Postmortem: OpenFangOps Agent Instability — MCP Tools, DB Manifest Injection, and Memory Architecture Ambiguity

**Date:** 2026-04-25
**Authors:** Cursor agent session
**Status:** Final — codebase follow-up completed 2026-04-25 (see Verification)
**Duration:** ~4 hours of active diagnosis and remediation
**Severity:** Operational — primary agent repeatedly lost tool access and exhibited inconsistent behavior

---

## Executive Summary

The newly created OpenFangOps agent experienced repeated cycles of losing MCP tool access, running with incorrect capabilities, and exhibiting bootstrap confusion despite workspace files being correctly populated. The root causes were: (1) the SurrealDB agent record injecting stale manifest values that overrode correct agent.toml template settings, (2) the OpenFang daemon systemd service lacking PATH entries for mise-managed tools, and (3) an architectural ambiguity in the memory layer where no single tier is designated authoritative for agent state.

The instability was first mitigated with a clean spawn, HTTP `emcp-global`, and systemd `PATH`. The **architectural** issue (stale DB manifest vs `agent.toml`) is now addressed in **openfang** `kernel.rs`: on boot, a valid `agent.toml` **fully replaces** the embedded manifest, then normalization runs; SurrealDB is updated as a cache. Bootstrap suppression also considers a filled name line in `USER.md` (see `prompt_builder`). Async persistence `mem::drop` anti-patterns on `save_agent` / session deletes are fixed.

---

## Timeline (all times UTC-7)

| Time | Event |
|------|-------|
| ~22:00 | openfang-config assessed as obsolete; decision to create OpenFangOps |
| ~23:00 | OpenFangOps created via wizard; workspace configured (VOICE.md, context.md, TOOLS.md, etc.) |
| ~23:20 | agent.toml fixed: temperature, DISCOVER protocol, empty `[capabilities]` block removed |
| ~00:00 | kill_agent DB deletion bug discovered and fixed in kernel.rs |
| ~00:15 | Built with `--features memory-candle`; all four Candle backends confirmed active |
| ~00:30 | DB wiped to clear ghost agents; daemon restarted |
| ~00:40 | OpenFangOps confirmed operational; API access working via TOOLS.md key |
| ~01:30 | MCP tools not appearing in agent; began diagnosis |
| ~02:30 | Root cause: `mcp_servers = []` (empty allowlist) + bunx/node not on daemon PATH |
| ~03:00 | PATH fix applied to systemd service; bun/bunx/node symlinked to ~/.local/bin/ |
| ~03:15 | ai-filesystem-mcp installed on prtr; both servers connected: 115 tools confirmed |
| ~03:40 | emcp-global SSE stream dropped; agent lost tools on re-session |
| ~03:50 | Switched emcp-global to HTTP transport; supergateway subprocess eliminated |
| ~04:09 | Daemon restarted; new daemon booted WITHOUT connecting MCP servers (PATH issue returned?) |
| ~04:20 | Agent reported zero MCP tools; stale DB capabilities discovered as separate cause |
| ~04:30 | Identified SurrealDB manifest injection as the primary instability source |
| ~04:35 | Killed agent, clean spawn from template; all capabilities correct |
| ~04:40 | Bootstrap fired despite USER.md having user's name; KV vs corefile conflict identified |
| ~05:00 | Architectural root cause articulated: no single source of truth for agent state |

---

## Root Cause Analysis

### What Happened

OpenFangOps lost MCP tool access multiple times across the session. Each time the symptom was the same — zero MCP tools — but the cause was different. The diagnosis loop continued because each fix exposed the next failure mode of the same underlying architectural issue.

### Contributing Causes (in order of discovery)

**1. Empty `mcp_servers = []` in agent.toml (initial)**
The wizard-created agent had an empty allowlist. The kernel generates an `mcp_summary` text block only for servers in the allowlist — empty list means no MCP tools described to the agent even if servers are connected globally. Fixed by populating the list.

**2. Daemon PATH missing mise-managed tools**
The systemd user service had no `Environment=PATH` line. `bunx` (for emcp-global's supergateway) and `node` (for ai-filesystem) are managed by mise and not on the default systemd PATH. MCP server subprocesses failed silently at boot. Fixed by adding PATH to the service file and symlinking bun/bunx/node into `~/.local/bin/`.

**3. emcp-global SSE stream instability**
The stdio+supergateway transport bridged an SSE stream to mcp.ism.la. SSE streams drop unpredictably (network blips, server timeouts). When the stream drops, the subprocess closes and MCP tools become unavailable until daemon restart. Fixed by switching to `type = "http"` transport — no subprocess, no SSE, direct HTTP to mcp.ism.la.

**4. SurrealDB agent record injecting stale manifest (primary)**
The DB record stores the full agent manifest. The boot diff-check reconciles only ~10 fields (name, description, system_prompt, provider, model, capabilities.tools, tool_allowlist, tool_blocklist, skills, mcp_servers). Fields NOT reconciled include: temperature, profile, exec_policy, and the full capabilities struct beyond `tools`. When the agent.toml was updated (capabilities block removed, profile = "full" added), the DB record kept stale values. The running agent had wrong tool access because the DB record overrode the template for fields the diff-check ignores. **This is the cause that produced the repeated instability** — every fix to the template was partially neutralized by the stale DB record.

**5. Memory architecture tier ambiguity (systemic, unfixed)**
The system has three independent tiers where "agent state" can live:
- Tier 1: agent.toml template (disk)
- Tier 2: SurrealDB agent record (substrate)
- Tier 3: Workspace files (disk, per-turn)

No tier is designated authoritative. The DB record was designed to persist the manifest across restarts, but agent.toml already provides that (it's on disk). The manifest in the DB is a redundant copy that becomes stale and competes with the template. Additionally, workspace corefiles (USER.md, MEMORY.md) and the agent's KV/semantic memory stores describe the same domain (user identity, persistent facts) with no sync and no declared winner. Bootstrap suppression reads KV only, ignoring USER.md — so USER.md having the user's name doesn't prevent the bootstrap from asking for it again.

---

## Detection

### What Worked
- The agent's self-reporting was accurate: it correctly stated it had zero MCP tools and identified possible causes
- Dashboard tools tab showed kernel-level tool count (177), which gave ground truth when agent reported zero
- Daemon logs showed MCP connection results clearly once we knew where to look

### What Didn't Work
- No alerting on MCP server connection failure at boot
- No observability into whether the running agent's manifest matches the template
- The diff-check produces no log output when fields are NOT updated, making it invisible

### Detection Gap
The core gap: there's no way to inspect what configuration a running agent is actually using without querying both the DB record and the agent.toml and comparing them manually. This makes it impossible to know whether a template change took effect without killing and respawning the agent.

---

## Impact

- OpenFangOps unavailable or degraded for the majority of a 4-hour setup session
- Multiple redundant diagnosis cycles for the same symptom (zero MCP tools) with different root causes
- Clean state required a DB wipe (losing all accumulated agent history) and multiple daemon restarts
- The bootstrap/USER.md conflict means the agent cannot reliably initialize user context across DB wipes

---

## Lessons Learned

### What Went Well
- The kill_agent bug was identified and fixed before it caused further problems
- Switching to HTTP transport for emcp-global is a genuine improvement — eliminates a fragile subprocess
- The PATH fix (systemd Environment line) is the right long-term approach
- 115 MCP tools operational by end of session

### What Went Wrong
- Each symptom was treated as an isolated bug rather than a manifestation of the same architectural root
- The SurrealDB manifest injection issue was not recognized until late in the session
- Multiple partial fixes were applied before the DB record was identified as the source of repeated failures
- Symlinking bun/bunx/node into `~/.local/bin/` violates the mise ownership model (technical debt)

### Where We Got Lucky
- The kill_agent fix was deployed before the final clean spawn, so the spawn actually cleared the DB record correctly
- HTTP transport for emcp-global worked immediately and is more stable than the supergateway approach

---

## Action Items

| Priority | Action | Status |
|---|---|---|
| P0 | Remove manual symlinks from `~/.local/bin/` (bun, bunx, node); ensure mise shims cover them | prtr ops (open) |
| P0 | Move ai-filesystem into eMCP on crtr as a container (emcp-add-server pattern) | crtr ops (open) |
| P1 | Boot reads full `agent.toml` when present; DB manifest is cache | **Done** (`kernel.rs`, tests) |
| P1 | Clarify memory tiers: manifest vs corefiles (prompt) | **Done** (AGENTS.md, canon `memory-intelligence.md`) |
| P2 | Add `mcp_connected_servers` to health/detail API response | `openfang-api` (open) |
| P2 | Boot observability: `source=disk` / `db_fallback` logging | **Done** |
| P2 | Remove `max_results` from `[web.searxng]` in config.toml (silently ignored field) | `~/.openfang/config.toml` (open) |
| P3 | Bootstrap: consider `USER.md` name line | **Done** (`prompt_builder.rs`) |
| P3 | Document manifest authority in AGENTS.md | **Done** |

## Verification (2026-04-25)

- `cargo test -p openfang-kernel --test agent_manifest_authority_test` — disk manifest wins over stale DB; DB fallback when `agent.toml` missing.
- `cargo test -p openfang-runtime --lib prompt_builder::tests::test_bootstrap_suppressed_when_user_md_has_name`
- `cargo build -p openfang-kernel` (full workspace build recommended before deploy).
- Runtime: edit a previously non-diffed field (e.g. `temperature` or `profile`) in `agent.toml`, restart daemon, confirm agent reflects change **without** delete/respawn.

---

## Architectural Note

**Update:** The kernel now treats `agent.toml` as authoritative for declarative manifest on boot (when the file exists and parses) and re-persists the normalized manifest to SurrealDB. The DB record remains valuable for **identity, sessions, and accumulated runtime state**; the embedded manifest is a **cache** aligned with disk at boot and after in-memory updates.

Corefiles remain prompt/reference (Layer 3), not a second manifest. KV and semantic memory still serve different stores; cross-tier consistency is an ongoing product concern but no longer conflated with `agent.toml` vs DB manifest drift.
