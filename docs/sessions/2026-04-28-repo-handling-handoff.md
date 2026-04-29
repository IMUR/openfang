# Session Handoff: 2026-04-28 Repo Handling

## Summary

Repository state was made safe after the memory architecture implementation left
`main` with a large dirty working tree. The full dirty state was snapshotted to
`edge`, `main` was reset clean to `origin/main`, and a fresh implementation
branch was created for review and commit cleanup.

## Completed This Session

- Verified `edge` had no commits not already present on `main`.
- Repointed `edge` to current `main`.
- Committed the full dirty implementation snapshot on `edge`:

```text
5ee99ff wip: snapshot memory architecture implementation
```

- Pushed the snapshot to `origin/edge`.
- Switched back to `main`.
- Reset local `main` clean to `origin/main`.
- Created a new branch from clean `origin/main`:

```text
memory-architecture-implementation
```

- Restored the `edge` snapshot into `memory-architecture-implementation` as an
  uncommitted working tree for later review/splitting.

## Current Repo State

Current branch:

```text
memory-architecture-implementation
```

Branch base:

```text
0164a7c docs(architecture): add SurrealDB practical architecture breakdown
```

Safety snapshot:

```text
edge / origin/edge = 5ee99ff wip: snapshot memory architecture implementation
```

`main` is clean and aligned with `origin/main`. The implementation branch is
dirty by design.

## Dirty Working Tree on `memory-architecture-implementation`

The branch currently contains uncommitted restored implementation changes across:

- repo guidance: `AGENTS.md`, `CLAUDE.md`
- API: `crates/openfang-api/src/rate_limiter.rs`, `routes.rs`, `server.rs`
- CLI: `crates/openfang-cli/Cargo.toml`, `src/main.rs`
- kernel/corefile generation: `crates/openfang-kernel/src/kernel.rs`
- memory substrate/datetime migration:
  - `crates/openfang-memory/src/consolidation.rs`
  - `crates/openfang-memory/src/db.rs`
  - `crates/openfang-memory/src/knowledge.rs`
  - `crates/openfang-memory/src/semantic.rs`
  - `crates/openfang-memory/src/session.rs`
  - `crates/openfang-memory/src/structured.rs`
  - `crates/openfang-memory/src/substrate.rs`
  - `crates/openfang-memory/src/usage.rs`
  - `crates/openfang-memory/tests/datetime_backfill.rs`
- runtime/types:
  - `crates/openfang-runtime/src/agent_loop.rs`
  - `crates/openfang-runtime/src/candle_classifier.rs`
  - `crates/openfang-runtime/src/drivers/mod.rs`
  - `crates/openfang-runtime/src/prompt_builder.rs`
  - `crates/openfang-runtime/tests/corefile_authorship.rs`
  - `crates/openfang-types/src/datetime.rs`
  - `crates/openfang-types/src/lib.rs`
  - `crates/openfang-types/src/memory.rs`
- corefile templates:
  - `docs/architecture/corefiles/AGENTS.md`
  - `docs/architecture/corefiles/BOOTSTRAP.md`
  - `docs/architecture/corefiles/HEARTBEAT.md`
  - `docs/architecture/corefiles/IDENTITY.md`
  - `docs/architecture/corefiles/SOUL.md`
  - `docs/architecture/corefiles/TOOLS.md`
  - `docs/architecture/corefiles/VOICE.md`
- research docs:
  - `docs/research/memory-architecture-exploration.md`
  - `docs/research/memory-architecture-exploration-rev2.md`
  - `docs/research/memory-system-analysis.md`
  - `docs/research/hybrid_memory_exploration.md`

There are also modified files that may be earlier-session carryover rather than
part of the core memory architecture implementation:

- `crates/openfang-kernel/tests/agent_manifest_authority_test.rs`
- `crates/openfang-memory/tests/audit_readonly.rs`
- `crates/openfang-migrate/src/openclaw.rs`

Review these carefully before committing.

## Recommended Commit Strategy

Avoid deep surgical splitting. This work has already been deployed and several
parts are coupled, especially the inspection API and datetime migration.

Recommended maximum split:

1. **Implementation commit**
   - memory inspection API/CLI
   - native datetime deserialization/write/backfill
   - corefile generation/test support
2. **Documentation commit**
   - canon references mirrored into repo guidance
   - corefile template comments
   - research docs and session docs

If review pressure is low, a single implementation commit is acceptable because
`edge` already preserves the exact full snapshot.

## Verification Already Completed

The deployed work was verified before this handoff:

```bash
cargo test -p openfang-runtime --test corefile_authorship
cargo test -p openfang-api --lib rate_limiter::tests::test_costs
cargo test -p openfang-memory --test datetime_backfill
cargo test -p openfang-memory --lib
cargo check -p openfang-cli
cargo build --release --features memory-candle -p openfang-cli
```

Runtime verification also passed:

```bash
openfang health --json
openfang memory inspect --agent OpenFangOps --scope semantic --classification-source candle --limit 2 --json
openfang memory inspect --agent OpenFangOps --graph --limit 2 --json
```

Final manual smoke marker:

```text
memory-architecture-final-smoke-manual-20260428
```

Inspection confirmed the marker was written as semantic memory with:

```text
classification_source = candle
scope = semantic
category = question
```

## Decisions Made

- **Use `edge` as a safety snapshot.**
  - Reason: the dirty working tree contained a large deployed implementation plus
    earlier drift. Snapshotting first made cleanup reversible.
- **Do not keep working on dirty `main`.**
  - Reason: `main` should remain a clean mirror of `origin/main`.
- **Restore the snapshot into a fresh feature branch.**
  - Reason: this allows commit cleanup without risking the only copy of the work.
- **Avoid over-splitting commits.**
  - Reason: the work is coupled and already deployed; over-splitting increases
    risk of malformed intermediate states and repeated retesting cost.

## Rejected

- **Clobbering `edge` without checking uniqueness.**
  - Rejected because it could have destroyed unknown work. `edge` was checked
    first and had no commits not already on `main`.
- **Resetting `main` before snapshotting.**
  - Rejected because it would risk losing the deployed implementation work.
- **Deep commit surgery as the default.**
  - Rejected because it would create avoidable risk without much benefit for
    this already-deployed bundle.

## Next Session Should

1. Stay on `memory-architecture-implementation`.
2. Decide whether to make one commit or two commits.
3. Inspect likely carryover files before staging:
   - `crates/openfang-kernel/tests/agent_manifest_authority_test.rs`
   - `crates/openfang-memory/tests/audit_readonly.rs`
   - `crates/openfang-migrate/src/openclaw.rs`
4. Re-run the focused verification set after committing.
5. Push the branch or open a PR when ready.

## Key Commands

```bash
git status --short --branch
git log --oneline --decorate -3 --all --branches=edge --branches=memory-architecture-implementation --branches=main
cargo test -p openfang-memory --test datetime_backfill
cargo test -p openfang-memory --lib
cargo check -p openfang-cli
```

## Key Takeaway

The repository is safe: `edge` holds the full remote snapshot, `main` is clean,
and `memory-architecture-implementation` contains the restored dirty tree ready
for a low-risk one- or two-commit cleanup.
