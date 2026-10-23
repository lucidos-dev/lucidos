# 0412: An Apply joins the running engine build instead of killing it; only the explicit Rebuild restarts it

- **Status**: Accepted
- **Date**: 2026-10-10

## Context

An engine-affecting Apply starts a background build in dev. A second Apply
used to abort that build and SIGKILL its process group, then start over. The
dev engine log held 1,160 build starts, and 180 of them died that way. Each
kill threw away a compile that was often most of the way done.

The abort also served as the only escape from a hung build, by accident.

## Decision

An Apply or a self-heal tick **joins** a build in flight. It leaves the build
alone, sets no state and emits no event. When the build finishes, the same
task checks whether its result covers HEAD. If a restart-requiring change sits
between them, the task builds once more. Only `POST /api/v1/engine/rebuild`
(the toast's Rebuild and Retry) still aborts and restarts.

The rules live in `engine::background_build`, generic over a `BuildHost`, so
they are tested over a stub build without cargo.

## Rationale

- **No compile is thrown away.** A burst of Applies during a build costs at
  most one follow-up build, however many Applies arrive.
- **The follow-up lives in the build task, not in self-heal.** The plan first
  leaned on the 10 s self-heal tick. Self-heal skips whenever the disk holds an
  upgrade over the running engine, and a build that finished one commit behind
  is exactly that. The user would then switch onto a binary that lacks the
  last Apply, and switch again later.
- **The shell's rebuild once stays.** `build_or_find_engine` rebuilds when HEAD
  moved during cargo, which absorbs a peer's Apply while this engine holds the
  checkout lock. The follow-up check reads the published binary's commit, so it
  never repeats work the shell already did.
- **The explicit Rebuild keeps the escape.** A hung build has no timeout, and
  the user needs one button that kills it.

## Consequences

- A build in flight runs to completion, even when an Apply lands mid-build.
- `build_state` stays `Building` across a follow-up, and the elapsed counter
  keeps counting from the first start.
- A follow-up after a failed build passes that failure on, so an identical
  repeat still reads as one and the toast can withhold Retry.
- An unknown answer from git reads as "not covered" and costs one more build,
  never a binary left behind HEAD.
- `trigger_background_rebuild` is gone. The two entry points are
  `join_or_start_background_rebuild` and `restart_background_rebuild`.

## Alternatives considered

- **Keep aborting, as before.** Simple, and the final binary always reflects
  the newest HEAD. It lost because 15% of builds died for nothing.
- **Rely on self-heal for the follow-up.** No new code path. It lost because
  self-heal skips when the disk holds any upgrade (see Rationale).
- **Hold merges while a build runs.** Every Apply, frontend ones included,
  would wait up to a whole build.
- **Build a frozen snapshot of the commit.** A second source path makes every
  build a cache miss (ADR 0392).
- **A build timeout instead of the explicit restart.** No good number exists
  for a build that takes 1 to 13 minutes, and the button already works.

Plan: `docs/plans/2026-10-10-apply-joins-the-running-engine-build.md`.
