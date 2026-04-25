# Session Handoff: 2026-04-25 Classifier Contract

## Summary

Completed the Agent State Authority deployment and the Candle classifier contract verification on prtr. The running daemon now uses `agent.toml` as the manifest authority, the OpenFangOps MCP workaround is persisted, and the DistilBERT classifier path has been rebuilt/deployed and verified with a real OpenFangOps turn plus memory metadata evidence.

---

## Completed This Session

- **Agent State Authority implementation and deployment**
  - `agent.toml` is now authoritative for declarative `AgentManifest` fields.
  - Boot restore replaces DB manifest cache from disk when `<home_dir>/agents/<name>/agent.toml` exists and parses.
  - Spawn and restore share `normalize_manifest_for_runtime()`.
  - Async memory persistence drops were removed from key update paths.
  - Bootstrap suppression contract was clarified around `USER.md` and KV.
  - Deployed with `cargo build --release --features memory-candle -p openfang-cli`.
  - Runtime logs confirmed `OpenFangOps` restored with `source="disk"`.

- **Stopped-daemon backups and DB reset**
  - Full stopped-daemon backup:
    `/home/prtr/.openfang-backups/pre-agent-state-authority-20260425-060246`
  - Active DB was later moved aside instead of deleted:
    `/home/prtr/.openfang-backups/pre-agent-state-authority-20260425-060246/openfang.db.cleared-20260425-063036`
  - Fresh DB booted, then `OpenFangOps` was spawned from its disk template.

- **OpenFangOps MCP recovery**
  - Found that `mcp_servers = ["emcp-global", "ai-filesystem"]` triggers a likely allowlist parser bug for hyphenated server names.
  - Operational workaround applied:
    `~/.openfang/agents/OpenFangOps/agent.toml` now has `mcp_servers = []`.
  - Live `OpenFangOps` runtime entry was updated through authenticated API to match.
  - Verified global tool registry includes:
    - `mcp_emcp_global_*`: 25 tools
    - `mcp_ai_filesystem_*`: 40 tools
    - total tools: 177

- **Bootstrap memory namespace correction**
  - `~/.openfang/workspaces/OpenFangOps/BOOTSTRAP.md` now stores:
    - `self.user_name`
    - `self.user_preference`
    - `self.first_interaction`
  - Canon docs clarified that `self.user_name` is private KV and not currently a global bootstrap suppression gate.

- **Classifier Contract Plan completed**
  - Current source already had the intended DistilBERT 4D U32 mask fix in `crates/openfang-runtime/src/candle_classifier.rs`.
  - `candle_classifier::tests::distilbert_mask_shape_contract` initially ran zero tests without features, then failed to compile under `memory-candle` because old `agent_loop` unit tests were missing newer feature-gated arguments.
  - Narrow test-call plumbing was fixed in `crates/openfang-runtime/src/agent_loop.rs`.
  - Feature-enabled focused test passed:
    `cargo test -p openfang-runtime --features memory-candle candle_classifier::tests::distilbert_mask_shape_contract`
  - Rebuilt release:
    `cargo build --release --features memory-candle -p openfang-cli`
  - Deployed rebuilt binary after backup:
    `/home/prtr/.openfang-backups/classifier-contract-20260425-102509`
  - Runtime health after deploy:
    - `memory_candle_binary: true`
    - `embedding_active: true`
    - `ner_active: true`
    - `reranker_active: true`
    - `classifier_active: true`
  - Controlled marker turn:
    `classifier-contract-20260425102542`
  - No post-deploy classifier fallback/broadcast/dtype warnings for that turn.
  - Memory metadata evidence found:
    - user directive fragment: `classification_source=rule`, `category=preference`, `scope=declarative`
    - turn summary fragment: `classification_source=candle`, `category=question`, `scope=semantic`

---

## Decisions Made

| Decision | Rationale |
| --- | --- |
| Treat DB backup as rollback, not as required migration | Clearing DB only worked before because it removed stale manifest cache; fixed boot logic should remove that need. |
| Keep `mcp_servers = []` as a runtime workaround | Empty mode means all connected MCP servers and bypasses the hyphenated-name allowlist bug. |
| Do not document the MCP workaround as canonical architecture | It is a temporary workaround for a parser bug, not desired steady-state behavior. |
| Verify classifier behavior with a real turn and metadata | `/api/health/detail` only proves the classifier loaded, not that inference worked. |
| Do not bundle health endpoint redesign into classifier fix | The fix was validated first; better health semantics remain a follow-up. |

---

## Current Runtime State

- `openfang.service` is running on prtr.
- Deployed binary is `~/.local/bin/openfang`.
- OpenFang health is OK.
- `OpenFangOps` is running.
- MCP global state is healthy enough for OpenFangOps:
  - `emcp-global`, `ai-filesystem`, `brave-search`, `github`, and `discord-mcp` connected.
  - `gmail` and `dropbox` still fail due missing credentials/files.
- The service unit warning about `daemon-reload` was handled during the classifier deployment.

---

## Files Modified This Session

### Repository

- `crates/openfang-kernel/src/kernel.rs`
  - Manifest restore from disk, normalization helper, async persistence bridge, boot logging.
- `crates/openfang-types/src/config.rs`
  - Added `spawn_default_assistant_on_empty_registry`.
- `crates/openfang-runtime/src/prompt_builder.rs`
  - Bootstrap suppression can consider `USER.md`.
- `crates/openfang-runtime/src/workspace_context.rs`
  - Added `VOICE.md` and `context.md` to scanned context files.
- `crates/openfang-runtime/src/candle_classifier.rs`
  - DistilBERT mask contract and entailment index handling already present and verified.
- `crates/openfang-runtime/src/agent_loop.rs`
  - Updated feature-gated test-call arguments so memory-candle tests compile.
- `crates/openfang-kernel/tests/agent_manifest_authority_test.rs`
  - Regression coverage for disk manifest authority and DB fallback.
- `AGENTS.md`
  - Manifest authority and async persistence guidance.
- `docs/sessions/2026-04-25-postmortem.md`
  - Updated with final agent-state fix and verification.
- `docs/sessions/2026-04-25-classifier-contract-handoff.md`
  - This handoff.

### Runtime / Canon / Records

- `~/.openfang/agents/OpenFangOps/agent.toml`
  - `mcp_servers = []` workaround persisted on disk.
- `~/.openfang/workspaces/OpenFangOps/BOOTSTRAP.md`
  - Stores `self.*` memory keys.
- `/mnt/ops/canon/openfang/memory-intelligence.md`
  - Clarified Layer 3/corefile prompt-context semantics and bootstrap gate behavior.
- `/mnt/ops/records/prtr-records/2026-04-25-agent-state-authority-deployment.md`
  - Deployment and decisions record.

Untracked repo files visible in `git status` at handoff time:

- `build.sh`
- `check.sh`
- `check_bg.sh`

These were not part of the classifier contract work and should be inspected before any commit.

---

## Discovered / Open Items

- **MCP allowlist parser bug**
  - `mcp_servers = ["emcp-global", "ai-filesystem"]` should work, but likely fails because naive server extraction parses `mcp_emcp_global_*` as `emcp` and `mcp_ai_filesystem_*` as `ai`.
  - Proper fix: use `extract_mcp_server_from_known()` in MCP allowlist filtering and summary generation.

- **Semantic memory observability gap**
  - Existing REST/CLI surfaces expose KV memory and backfill, not read-only semantic memory fragments.
  - Metadata verification required indirect non-locking WAL string inspection.
  - Add a proper read-only semantic memory inspection endpoint before relying on this operationally.

- **Health semantics gap**
  - `classifier_active` means “classifier object loaded,” not “last inference succeeded.”
  - Follow-up should expose loaded vs working/degraded, last success, and last error.

- **`rtr-openfang` runtime skill parse error**
  - Daemon logs:
    `Failed to load skill at /home/prtr/.openfang/skills/rtr-openfang: TOML parse error at line 1, column 1`
  - This is separate from the repo-level skill file and should be investigated before assuming OpenFangOps has the `rtr-openfang` skill context.

- **Candle embedding long-input edge**
  - Earlier logs showed one embedding failure on long input:
    `index-select invalid index 512 with dim size 512`
  - Not part of the classifier fix; likely requires tokenizer truncation contract review for embeddings.

- **Gmail / Dropbox MCP credentials**
  - `gmail` fails due missing `gcp-oauth.keys.json`.
  - `dropbox` fails due missing `DROPBOX_ACCESS_TOKEN`.

---

## Next Session Should

1. Fix MCP allowlist parsing for hyphenated server names and add regression coverage.
2. Add read-only semantic memory inspection API for memory metadata and classification verification.
3. Improve `/api/health/detail` memory intelligence semantics: loaded vs working/degraded.
4. Investigate `/home/prtr/.openfang/skills/rtr-openfang` TOML parse error.
5. Inspect untracked `build.sh`, `check.sh`, and `check_bg.sh` before committing or cleaning.

---

## Key Commands / Evidence

```bash
cargo test -p openfang-runtime --features memory-candle \
  candle_classifier::tests::distilbert_mask_shape_contract

cargo build --release --features memory-candle -p openfang-cli

curl -fsS -H "Authorization: Bearer <api_key>" \
  http://127.0.0.1:4477/api/health/detail
```

Runtime marker used for classifier verification:

```text
classifier-contract-20260425102542
```

Key result:

```text
classification_source=candle
category=question
scope=semantic
```

## Key Takeaway

Agent-state authority is deployed, OpenFangOps is operational with MCP tools via the all-server workaround, and the DistilBERT classifier path is now functionally verified beyond load-time health; the remaining work is observability and parser hardening, not another classifier patch.
