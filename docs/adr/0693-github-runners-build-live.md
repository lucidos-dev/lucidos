# 0693: A GitHub runner builds live, never in a launch tree, and a slow build prints a notice

- **Status**: Accepted
- **Date**: 2026-11-14
- **Amends**: [0668](0668-engine-build-compiles-a-pinned-snapshot.md)

## Context

ADR 0668 builds a primary checkout's engine in a launch tree, with its own
target dir. A GitHub runner's checkout is a primary checkout. So from 12 Nov
13:08 every e2e shard build compiled in `target/launch-tree/target`. It ignored
the target state the build job had just restored into `target/e2e/`, and the
dependency cache, and compiled all 652 crates.

| GitHub job | Median before | Median after |
|---|---|---|
| build-linux | 7.0 min (99 builds) | 21.5 min (45 builds) |
| build-macos | 14.8 min (70 builds) | 35.3 min (35 builds) |

Every GitHub e2e run and every main check paid it for two days. Nothing
noticed, because nothing watched build time.

It also poisoned the target states. The build job still packed `target/e2e/`
from the checkout, which now held only the older outputs it had restored. It
labelled them with the manifest of the newer build. Restoring such a state
ages every unchanged-looking input, so cargo reuses stale crates and the build
fails to compile.

## Decision

`LUCIDOS_LAUNCH_TREE=off` makes `launch_tree_wanted` false, so a primary
checkout builds live. `scripts/e2e-github-shard.sh` exports it for every
command it runs.

The target state's manifest is renamed `lucidos-target-state-v2`. A state
without it is a poisoned one, and the build deletes what it unpacked and
compiles from scratch. The first build after this change pays one cold compile
per platform, then uploads a sound state.

The GitHub driver also compares each build job with the median of its last 20
successful runs in the timings log. A job over twice that median prints a
notice, once five runs exist.

## Rationale

A launch tree exists because an Apply, a landing or follow-main moves HEAD
under a running cargo. Nothing moves a runner's HEAD: it checks out one commit
and is thrown away. So the launch tree buys a runner nothing, and costs it the
restored state, which lives in `target/` by design (ADR 0536).

An explicit switch beats detecting CI. A `GITHUB_ACTIONS` test would also turn
the tree off for `install-smoke`'s source install, which tests a stranger's
real path. The switch keeps that choice with each caller.

The notice is a ratio to the job's own history, so it needs no tuned threshold
per platform. Doubling is far outside normal spread, and the regression it
would have caught tripled the build.

## Consequences

- A GitHub shard build compiles in the profile directory its target state
  restores into. `launch_tree_test.sh` and `e2e_github_test.sh` pin both
  halves.
- Local primary checkouts keep the launch tree. Linked worktrees still build
  live.
- `install-smoke.yml`'s source-install leg keeps the launch tree, because it
  runs `install.sh --dev` exactly as a user does. Its dependency cache restores
  into `target/` and goes unused there. That leg's 40 minute health budget
  was sized for a cold build, so it still fits. `release-tarballs.yml` builds
  through `build-headless.sh`, which never used a launch tree.
- The notice is a line on the driver's stderr, not a failure. A build can be
  slow for a good reason, such as a toolchain bump.

## Alternatives considered

- **Detect a runner in `launch_tree_wanted`.** Rejected: it would also change
  the source-install smoke test, which should test what a user runs.
- **Restore the target state into the launch tree.** Rejected: the tree is
  recreated whenever its directory is missing, which wipes what was restored.
  It also adds a moving part to buy nothing on a checkout that never moves.
- **Fail the run on a slow build.** Rejected: a slow build is a cost, not a
  wrong verdict. The notice puts it in front of a reader without blocking one.
