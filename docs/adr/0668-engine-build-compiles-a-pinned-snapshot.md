# 0668: An engine build compiles a pinned snapshot in a launch tree, once

- **Status**: Accepted
- **Date**: 2026-11-12
- **Amends**: [0412](0412-apply-joins-the-running-engine-build.md)

## Context

`build_or_find_engine` compiled the live checkout. An Apply, a landing or
follow-main rewrote that tree under a running cargo, so the binary could mix
two commits. The shell then threw the build away and compiled again, and the
engine read exit 75 as one more reason to build. On the cloud box a 14 min
build became 28 min. At about 269 commits a day, HEAD moving mid-build is the
normal case.

## Decision

A primary working tree builds its engine set in a **launch tree**: a detached
git worktree at `target/launch-tree/` with its own target dir. Inside the build
slot, just before cargo, the build snapshots the working tree (uncommitted and
untracked files included) into the launch tree and compiles that. One lock per
launch tree covers the snapshot through the publish.

The shell never rebuilds and never exits 75. It publishes the snapshot's binary
and prints a note when HEAD moved past it. The engine keeps ADR 0412's single
follow-up, and treats a binary newer than the start HEAD as delivered.

A linked worktree still builds live: only its own session moves its HEAD.
**Amended by [ADR 0693](0693-github-runners-build-live.md):** so does a GitHub
runner, through `LUCIDOS_LAUNCH_TREE=off`.

## Rationale

- **Git never moves the launch tree under cargo.** Only the build itself
  writes it, under the lock, so the binary is built from one source state.
- **The stable path keeps cargo incremental.** `git read-tree -u --reset`
  rewrites only changed files, so unchanged files keep their mtimes.
- **A late snapshot coalesces Applies.** A build queued behind the slot or
  the memory gate picks up every Apply that landed while it waited.
- **Its own target dir is required.** Cargo's freshness check is mtime-based.
  Two source trees sharing one target can publish a stale artifact.

## Consequences

- One compile per build. An Apply that lands mid-compile and touches engine
  source still needs one follow-up build, run by the engine.
- One more target dir per stack checkout. The old `target/<profile>` launch
  outputs go unused and nothing deletes them.
- The first build after this change is cold for workspace crates. sccache
  still serves the registry crates.
- Codegen renders through the launch tree into the shared cache, and the live
  checkout syncs from it.
- This supersedes two parts of ADR 0412: its bullet "the shell's rebuild once
  stays", and its rejected "frozen snapshot" alternative. That rejection
  assumed a fresh path per build. A stable path keeps the cache.

## Alternatives considered

- **Share `target/` between the live checkout and the launch tree.** No extra
  disk. It lost because cargo compares mtimes, and an artifact built from one
  tree can look fresh for the other.
- **Hold merges while a build runs.** Every Apply would wait up to a whole
  build (ADR 0412).
- **A fresh temp path per build.** No lock needed. Every build would miss the
  incremental and sccache caches (ADR 0392).
- **Keep the rebuild once.** It throws a finished compile away whenever HEAD
  moves, which is most builds.

Plan: `docs/plans/2026-11-12-engine-build-compiles-a-pinned-snapshot.md`.
