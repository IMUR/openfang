---
name: OpenFang Config Audit
overview: Configuration quality audit and remediation for the OpenFang deployment at ~/.openfang on prtr. Audit complete; now executing user-approved fixes.
todos:
  - id: step1-untrack-secrets
    content: "P0: git rm --cached secrets.env .env, fix .gitignore, commit in ~/.openfang"
    status: completed
  - id: step2-show-struct
    content: "P1 gate: grep KernelConfig struct from surreang branch, show user before rewriting providers"
    status: completed
  - id: step3-fix-env-openai
    content: "P1: Remove OPENAI_API_KEY from .env entirely (wrong value — Zhipu key)"
    status: completed
  - id: step4-fix-hf-key
    content: "P1: grep code for HF_API_KEY vs HUGGINGFACE_API_KEY, rename in secrets.env to match code"
    status: completed
  - id: step5-telegram-acl
    content: "P1: Add allowed_users = [] to [channels.telegram] in config.toml"
    status: completed
  - id: step6-default-model
    content: "P1: Add api_key_env = PERPLEXITY_API_KEY to [default_model] (do NOT change model)"
    status: completed
  - id: step7-restart-daemon
    content: "Deploy: cargo build --release, openfang stop, cp release binary, openfang start"
    status: completed
  - id: step8-verify
    content: "Verify: curl health/detail, check for config_warnings and DDL errors"
    status: completed
isProject: false
---

# OpenFang Configuration Audit + Remediation -- prtr Node

## Current State (updated per 2026-03-29 handoff revision)

- **Daemon:** v0.5.1 release build, DDL already applied, 5 agents, Telegram connected
- **Binary:** Release build (82MB) copied to `~/.local/bin/openfang` (not a symlink)
- **Branch:** `surreang` (local only, 3 commits ahead of main)
- **SurrealDB:** v2.6.5 embedded SurrealKV, ns=`openfang`, db=`agents`, schema DDL applied
- **Deployment pattern:** `cargo build --release` then `cp target/release/openfang ~/.local/bin/openfang`

## Execution Plan

### Step 1 -- Untrack secrets + fix .gitignore + commit

In `~/.openfang`:

```bash
git rm --cached secrets.env .env
```

Replace `.gitignore` contents with:

```
data/
*.env
secrets.env
daemon.json
tui.log
cron_jobs.json
```

Then:

```bash
git add .gitignore
git commit -m "untrack secrets, fix gitignore"
```

No key rotation -- no remote exists, nothing pushed.

### Step 2 -- Show KernelConfig struct (gate for provider rewrite)

```bash
grep -A 30 "struct KernelConfig" ~/prj/openfang/crates/openfang-kernel/src/*.rs
```

If the struct does NOT have `provider_urls` / `provider_api_keys` fields, the `[providers.*]` blocks may actually be parsed by a different mechanism on `surreang`. Must confirm before rewriting config.toml.

**STOP and show user the output before proceeding.**

### Step 3 -- Fix .env OPENAI_API_KEY

Remove the `OPENAI_API_KEY` line from `~/.openfang/.env` entirely. The real OpenAI key already lives in `secrets.env` as `OPENAI_API_KEY=sk-proj-...`. If it's missing from secrets.env, report to user.

### Step 4 -- Fix HF key name mismatch

```bash
grep -rn "HF_API_KEY\|HUGGINGFACE_API_KEY" ~/prj/openfang/crates/
```

Whichever name the code reads, rename the env var in `secrets.env` to match. Do NOT change the code.

### Step 5 -- Telegram access control

Add to `~/.openfang/config.toml`:

```toml
[channels.telegram]
allowed_users = []
```

Leave empty -- user will fill in their Telegram user ID.

### Step 6 -- Explicit api_key_env on default model

Change `[default_model]` in config.toml to:

```toml
[default_model]
model = "sonar-basic"
provider = "perplexity"
api_key_env = "PERPLEXITY_API_KEY"
```

Do NOT change the model or provider. This is intentional for cost management.

### Commit steps 3-6

Commit all config changes in `~/.openfang` after steps 3-6.

### Step 7 -- Restart daemon

No code changes were made -- only config files in `~/.openfang`. The binary reads `config.toml` at boot time. Just restart:

```bash
openfang stop
openfang start
```

No build, no copy. The release binary at `~/.local/bin/openfang` is unchanged. Save the full four-step deployment pattern (`cargo build --release` + `cp`) for when there are actual Rust code changes to compile.

### Step 8 -- Verify

Primary check -- OpenFang health API:

```bash
curl -s http://127.0.0.1:4200/api/health/detail
```

Check for:

- `config_warnings` array (should be empty or reduced)
- `database: "connected"`
- `agent_count` restored

Note on systemd: `openclaw-gateway.service` is a **separate** Node.js OpenClaw gateway on port 4444 -- it is NOT OpenFang. No OpenFang systemd unit exists; OpenFang runs as a direct user process. `journalctl --user -u openclaw-gateway.service` shows OpenClaw logs only.

### Incidental finding -- Telegram bot conflict

OpenClaw (port 4444) and OpenFang (port 4200) are both polling the same `TELEGRAM_BOT_TOKEN`, causing 409 conflicts in the OpenClaw logs ("terminated by other getUpdates request"). Out of scope for this plan, but should be resolved by either disabling Telegram in OpenClaw or using separate bot tokens.

## Explicitly Deferred (per user)

- Key rotation (no remote, not urgent)
- Provider deduplication (kimi/moonshot and zai/zhipu are intentional)
- Release build switch (already done per handoff)
- API auth (`api_key`)
- Integration cleanup
- `memory.decay_rate` change (0.05 is intentional)
- Default model change (perplexity/sonar-basic is intentional)
- Telegram bot token conflict between OpenClaw and OpenFang

