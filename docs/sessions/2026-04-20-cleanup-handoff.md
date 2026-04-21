# Session Handoff: 2026-04-20 — v0.6.0 Cleanup

**Scope:** cleanup only. All code work from the v0.6.0 upstream merge is landed, pushed, and running. See `docs/sessions/2026-04-20-handoff.md` for the merge itself.

**Assumed prior state:** `main = 8b57e6a`, pushed to `origin`; `v0.6.0` tag on origin; `openfang.service` active at v0.6.0.

---

## Summary

Five categories of post-landing residue to clean up, staged by readiness:

1. **Filesystem backup artifacts** (gated: wait for a workload cycle)
2. **Local backup branches** (gated: same as above)
3. **Stale local branches** (ready now, needs per-branch decision)
4. **Tag gap on origin** (ready now, optional)
5. **Untracked research material** (user decision, unrelated to v0.6.0 correctness)

Nothing here is blocking. All of it can sit indefinitely without breaking anything.

---

## 1. Filesystem backup artifacts

Safety-net copies taken immediately before the v0.6.0 binary install.

| Path | Size | Purpose |
|---|---|---|
| `~/.openfang.bak-v0.6.0/` | 1.2 GB | Full KV/sessions/models/config snapshot |
| `~/.local/bin/openfang.bak-pre-v0.6.0` | 154 MB | Pre-merge daemon binary |
| `~/.config/systemd/user/openfang.service.bak-pre-v0.6.0` | <1 KB | Pre-merge unit file |

**Gate before deletion:** at least one full real workload cycle (24-48h) with v0.6.0 active — scheduled cron deliveries firing, hand agents completing turns, voice sessions if any. If anything regresses, the rollback sequence is:

```bash
systemctl --user stop openfang.service
cp ~/.local/bin/openfang.bak-pre-v0.6.0 ~/.local/bin/openfang
cp ~/.config/systemd/user/openfang.service.bak-pre-v0.6.0 \
   ~/.config/systemd/user/openfang.service
systemctl --user daemon-reload
# Optional: restore data snapshot if corruption suspected
# rm -rf ~/.openfang/data && cp -a ~/.openfang.bak-v0.6.0/data ~/.openfang/data
systemctl --user start openfang.service
```

**When the gate passes, cleanup is:**

```bash
rm -rf ~/.openfang.bak-v0.6.0
rm ~/.local/bin/openfang.bak-pre-v0.6.0
rm ~/.config/systemd/user/openfang.service.bak-pre-v0.6.0
```

**Sanity check before running:**

```bash
systemctl --user is-active openfang.service   # → active
curl -s http://127.0.0.1:4477/api/health      # → status:ok, version:0.6.0
curl -s http://127.0.0.1:4477/api/health/detail | python3 -c \
  'import json,sys; d=json.load(sys.stdin); print("panics:", d["panic_count"], "restarts:", d["restart_count"])'
# Want both counters at 0 or at least stable over the observation window.
```

---

## 2. Local backup branches

| Branch | HEAD (at time of backup) | Purpose |
|---|---|---|
| `backup/main-pre-upstream-v0.6.0` | pre-merge `main` | Rollback point for this merge |
| `backup/main-pre-upstream-2026-04-16` | older pre-merge `main` | Earlier merge attempt checkpoint |

Gated on the same workload cycle as §1. Delete with:

```bash
git branch -D backup/main-pre-upstream-v0.6.0
git branch -D backup/main-pre-upstream-2026-04-16
```

Use `-D` (capital) — these branches aren't reachable from `main` so plain `-d` will refuse.

**Before deleting:** confirm the HEAD commit of each is also reachable somewhere you trust:

```bash
git log -1 --format='%h %s' backup/main-pre-upstream-v0.6.0
git branch -a --contains $(git rev-parse backup/main-pre-upstream-v0.6.0)
# If only the backup branch contains it and you've never needed to check it
# out, it's safe to drop. Otherwise you're about to make commits unreachable.
```

---

## 3. Stale non-backup branches

Full list at handoff time:

```
backup/main-pre-upstream-2026-04-16   (see §2)
backup/main-pre-upstream-v0.6.0       (see §2)
dev
edge
merge/upstream-0.5.5-tier-a
surreang
```

### `merge/upstream-0.5.5-tier-a`

Superseded by the v0.6.0 merge that just landed. Check before deleting:

```bash
git log -1 --format='%h %s' merge/upstream-0.5.5-tier-a
git merge-base --is-ancestor merge/upstream-0.5.5-tier-a main && \
  echo "fully merged into main, safe to delete with -d" || \
  echo "NOT fully merged; review before using -D"
```

If fully merged: `git branch -d merge/upstream-0.5.5-tier-a`.
If not: inspect what's unique (`git log main..merge/upstream-0.5.5-tier-a --oneline`) and decide.

### `dev`, `edge`, `surreang`

Unknown semantics from this session. **Do not delete without inspection.**

For each:

```bash
git log -1 --format='%h %ad %s' --date=short <branch>
git log main..<branch> --oneline | head -20     # unique commits
git branch --contains <branch> | head -5         # is it under anything?
```

Likely candidates (to confirm with the operator who created them):
- `surreang` — probably the SurrealDB migration working branch
- `dev` / `edge` — generic channels; may be long-lived integration branches or abandoned experiments

Do not touch these without explicit approval.

---

## 4. Tag gap on `origin`

Local has v0.5.8, v0.5.9, v0.5.10, v0.6.0. Origin currently has up through v0.5.7 plus v0.6.0 (pushed in the landing session). Gap:

```
v0.5.8   on local, not on origin
v0.5.9   on local, not on origin
v0.5.10  on local, not on origin
```

These are upstream tags fetched via `git fetch upstream`. Pushing them to origin is optional housekeeping — it gives `origin` the same tag surface as local / upstream, which is useful for `git describe` and changelog tools.

```bash
git push origin v0.5.8 v0.5.9 v0.5.10
```

Do **not** use `git push origin --tags` — that pushes every local tag, which is fine today but is a pattern that will one day surprise someone.

Verify after:

```bash
git ls-remote --tags origin 'v0.5.*' | sort
```

---

## 5. Untracked research material

Working tree currently has:

```
?? docs/research/2601.07372v1.pdf
?? docs/research/Architecture Guide_ Implementing AutoDream Memory Consolidation for AI Agents.md
```

Not cleanup — this is a pending user decision from earlier in the session. Options:

- **Commit** them under `docs/research/` (the intended location, alongside existing research material there). Would be a `docs(research):` commit.
- **Gitignore locally** if the user wants them on disk but not tracked.
- **Delete** if they've served their inspirational purpose.

No correctness impact either way.

---

## Verification after any cleanup step

Same health gates as before:

```bash
systemctl --user is-active openfang.service
curl -s http://127.0.0.1:4477/api/health
curl -s http://127.0.0.1:4477/api/status | python3 -c \
  'import json,sys; d=json.load(sys.stdin); print(f"{d[\"agent_count\"]} agents,", \
   sum(1 for a in d["agents"] if a["state"]=="Running"), "running")'
git status --short
git branch
```

If any of those regress after a cleanup action, stop — the backups in §1 and §2 are the rollback path, so they should be the last thing cleaned.

---

## Recommended execution order

1. **Now:** §3 — delete `merge/upstream-0.5.5-tier-a` if it's an ancestor of `main`. Leave `dev`/`edge`/`surreang` alone pending owner input.
2. **Now (optional):** §4 — push v0.5.8/9/10 to origin for tag parity.
3. **After 24-48h of stable v0.6.0 operation:** §1 + §2 together. The filesystem backup and the backup branches are the same rollback mechanism at different layers; they should retire together.
4. **Whenever:** §5 — user decision on research docs.

---

## Anti-patterns to avoid

- **Deleting filesystem backups before the systemd service has run a full cron delivery cycle.** Schedule migrations only surface their state under real cron fires. A 10-minute smoke test isn't the same as a day of real scheduling.
- **`git push origin --tags`.** Pushes everything indiscriminately including any experimental or historical tags that may exist locally.
- **`git branch -D`** on anything not under `backup/` or `merge/` without first listing its unique commits.
- **Regenerating `crates/openfang-desktop/gen/schemas/*.json`** during release builds is expected — tauri does this. Always `git checkout --` those three files rather than committing them. They're tracked but effectively build artifacts.

---

## Files referenced in this handoff

- `docs/sessions/2026-04-20-handoff.md` — the merge itself (committed this session)
- `docs/architecture/memory-intelligence.md` — refreshed to v0.6.0 (committed this session)
- `docs/architecture/voice-intelligence.md` — refreshed to v0.6.0 (committed this session)

## Commits on `main` from the v0.6.0 landing

```
8b57e6a docs(architecture): refresh memory + voice intelligence for v0.6.0
8b86567 docs(sessions): add v0.6.0 upstream merge handoff
15f8ee7 Merge remote-tracking branch 'upstream/main' into merge/upstream-v0.6.0
```

All pushed to `origin/main`. `v0.6.0` tag pushed to origin.
