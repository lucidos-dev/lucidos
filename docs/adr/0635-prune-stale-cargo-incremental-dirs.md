# 0635: The build wrapper prunes stale cargo incremental crate dirs

- **Status**: Accepted
- **Date**: 2026-11-09

## Context

rustc keeps one `target/<profile>/incremental/<crate>-<hash>/` dir per crate and
build configuration: plain build, test, clippy, feature set, toolchain,
RUSTFLAGS. Cargo clears the session dirs inside a crate dir. It never removes a
crate dir whose configuration stopped being built.

The main checkout of the maintainer's machine reached 435 dirs and 110 GB, with
105 GB untouched for a week. Each worktree target adds 20 to 40 GB. The disk
passed 90 % full, and the snapshot disk floor refused captures.

Pinned stable cargo (1.94) has no target-dir gc. `cargo clean` has no gc mode,
and nightly `-Zgc` cleans only the `~/.cargo` registry and git caches.

Plan: [`docs/plans/2026-11-09-prune-stale-cargo-incremental-cache.md`](../plans/2026-11-09-prune-stale-cargo-incremental-cache.md).

## Decision

`scripts/with-build-slot.sh` runs `prune_stale_incremental` after the memory
gate and before the slot, at most once a day per build dir. It moves each
crate dir with an mtime older than 7 days into its run's own trash dir,
`<build>/.stale-incremental.<pid>/`. It deletes that in the background. So
no two runs share a tree. It skips a profile dir while another
cargo holds its `.cargo-lock`, and it never fails or delays the build.

## Rationale

- **The wrapper sees every build.** `make lint`, `test-engine.sh`, the dev and
  Apply engine builds, codegen and the build measurement all go through it. It
  runs in the main checkout and in every worktree, with or without an engine.
- **Move, then delete.** A rename on one volume is atomic, so an interrupted
  delete leaves nothing at the original path. A run removes a trash dir
  untouched for a day. The first prune of a 100 GB backlog never holds a build.
- **Same liveness rule as ADR 0408.** `.cargo-lock` says whether a build runs.
  `lsof` reads it, and perl's flock where a minimal Linux image has no `lsof`.
  Both prunes share `scripts/lib/cargo_build_lock.sh`. The moves also hold
  that lock, so no cargo starts between the check and a move.
- **Cargo names the build dir.** `cargo metadata` reports `build_directory`,
  which honours `build.build-dir`, `CARGO_TARGET_DIR` and `build.target-dir`.
- **Pruning a live unit costs one non-incremental compile** of that crate, on
  its next change.

## Consequences

- This is a temporary measure, in `docs/temporary-measures.md`. It ends when
  stable cargo prunes unused build-dir units itself.
- A host with neither `lsof` nor perl keeps its cache. The prune says so once
  a day.
- Stale `deps/` artifacts from old configurations are not covered.

## Alternatives considered

- **The `WorktreeCleanup` worker.** Rejected: it owns a workspace's worktrees
  only and never sees the main checkout, where the backlog was largest. Tier 1
  already strips a whole worktree target (ADR 0311, ADR 0035).
- **Cargo's own gc.** Rejected: stable cargo has none for a target dir.
- **A periodic `cargo clean`.** Rejected: it costs every user a full rebuild
  to reclaim space the stale dirs alone hold.
