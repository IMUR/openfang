# Postmortem: OpenFangOps Agent Instability — MCP Tools, DB Manifest Injection, and Memory Architecture Ambiguity

**Date:** 2026-04-25
**Authors:** Cursor agent session
**Status:** Final
**Duration:** ~4 hours of active diagnosis and remediation
**Severity:** Operational — primary agent repeatedly lost tool access and exhibited inconsistent behavior

---

## Executive Summary

The newly created OpenFangOps agent experienced repeated cycles of losing MCP tool access, running with incorrect capabilities, and exhibiting bootstrap confusion despite workspace files being correctly populated. The root causes were: (1) the SurrealDB agent record injecting stale manifest values that overrode correct agent.toml template settings, (2) the OpenFang daemon systemd service lacking PATH entries for mise-managed tools, and (3) an architectural ambiguity in the memory layer where no single tier is designated authoritative for agent state.

The instability was resolved through a clean agent spawn (bypassing the stale DB record), switching emcp-global to HTTP transport (eliminating the fragile SSE subprocess), and adding PATH configuration to the systemd service. However, the underlying architectural issue remains unfixed in the codebase.

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

| Priority | Action | Location |
|---|---|---|
| P0 | Remove manual symlinks from `~/.local/bin/` (bun, bunx, node); ensure mise shims cover them | prtr ops |
| P0 | Move ai-filesystem into eMCP on crtr as a container (emcp-add-server pattern) | crtr ops |
| P1 | Fix: agent boot should always read agent.toml as authoritative, not the DB record | `kernel.rs` boot sequence |
| P1 | Define which memory tier owns which domain (user identity, persistent facts, behavior) | architecture decision |
| P2 | Add `mcp_connected_servers` to health/detail API response | `openfang-api` |
| P2 | Log which manifest fields were updated by the diff-check at each boot | `kernel.rs` |
| P2 | Remove `max_results` from `[web.searxng]` in config.toml (silently ignored field) | `~/.openfang/config.toml` |
| P3 | Bootstrap suppression should read USER.md or corefile, not only KV | `kernel.rs` prompt builder |
| P3 | Document the manifest DB vs template authority model explicitly in AGENTS.md | `AGENTS.md` |

---

## Architectural Note

The most significant finding of this session is not any individual bug but a structural ambiguity: the SurrealDB agent record stores a manifest that competes with agent.toml, and the memory system has three tiers (corefile, KV, semantic memory) covering the same domain with no designated winner.

The manifest in the DB serves no purpose that agent.toml doesn't already serve — agent.toml is on disk and survives restarts. The only things the DB genuinely needs to own are accumulated runtime state: session history, memories, and KV data. If the manifest were not stored in the DB record (or were treated as a strictly derived cache that agent.toml always overrides), the entire class of stale-DB-manifest failures disappears.

This is worth an architecture decision record before the agent ecosystem grows.
