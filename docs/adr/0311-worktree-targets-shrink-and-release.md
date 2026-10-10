# 0311: Worktree build targets: never a shared CARGO_TARGET_DIR; lean debuginfo; a thread with nothing pending releases its build artifacts

- **Status**: Accepted
- **Date**: 2026-09-27

## Context

Every coding-agent worktree builds its own cargo `target/`, and a worktree that
has run `/harden` and e2e holds 15 to 24 GB. Fourteen worktrees reached about
200 GB in four hours and filled the disk twice. Postgres, Docker and a frontend
build all failed.

Three things let the total grow:

- Each target carried full debuginfo for every crate.
- The retention gate kept an applied or abandoned thread's `target/` until the
  thread was archived or free disk fell below 20 GB.
- Under soft pressure, Tier 1 still waited for 24 h of idle. Only below 5 GB
  did that drop to 1 h, and the worker ran every 15 minutes.

Plan: [`docs/plans/2026-09-27-worktree-targets-stop-multiplying-disk.md`](../plans/2026-09-27-worktree-targets-stop-multiplying-disk.md).

## Decision

Keep one `target/` per checkout, and make each one smaller and shorter-lived:

1. The workspace `dev` profile builds our crates with `line-tables-only`
   debuginfo and dependencies with none.
2. A thread with nothing pending releases its build artifacts (Tier 1) after
   1 h idle, whatever the free disk. Nothing pending means no live session, no
   pending change, no owed parent fan-in, and not saved.
3. The disk monitor wakes the cleanup worker when pressure worsens. Under soft
   pressure the Tier 1 idle window is 1 h.

## Rationale

- **A shared target dir silently links the wrong code.** Cargo judges a
  workspace crate fresh by mtime, and two checkouts share one `-C metadata`
  hash (ADR 0079). Checkout `b`'s untouched files are older than checkout `a`'s
  last build, so `b` links `a`'s crate. We reproduced it: `b` printed `a`'s
  edit while `b`'s source held the old text, with no error and no rebuild.
- **Debuginfo is the cheapest size to cut.** The engine test build went from
  5.6 GB to 3.6 GB. Most of the saving is the `.o` files macOS keeps for
  unpacked debuginfo. Panics and backtraces keep file and line for our crates.
- **Stripping `target/` is not the harm the warm policy guarded against.**
  That policy stopped Tier 0 from removing a worktree and its branch under a
  thread the user kept returning to. Tier 1 leaves the worktree, its source and
  its branch. The cost of stripping is a cold rebuild on reopen, against 15 to
  24 GB held until then.
- **Every unanswered lookup keeps the artifacts.** A failed pending-change,
  saved or fan-in query counts as something pending, the same stance as the
  other tiers.
- **Wake on worsening, not on every tick.** A wake per tick would run a full
  cycle every minute through a long low-disk episode. The wake is an in-memory
  `Notify`, so the monitor still never waits on the database (ADR 0302).

## Consequences

- Concurrent worktrees still each hold a target, but a smaller one. A thread
  keeps it only while something is pending or it was active in the last hour.
- Reopening a thread idle over an hour with nothing pending rebuilds its
  target. `sccache` makes the dependency part of that rebuild cheap when it
  works.
- A debugger shows no local variables in our crates and nothing in
  dependencies. A developer who needs that overrides the profile locally.
- Existing targets, including the main checkout's, keep their old
  full-debuginfo artifacts until someone runs `cargo clean`. Cargo never
  garbage collects a target.

## Alternatives considered

**A shared `CARGO_TARGET_DIR`.** Rejected for the wrong-binary hazard above. It
would also serialize every concurrent session on cargo's build lock. Scripts
that read `<checkout>/target/...` would need to follow it too.

**A small pool of target dirs.** Rejected. Every handoff between checkouts
carries the same mtime hazard, and the pool still serializes the sessions that
share a slot.

**Seed each target with an APFS `clonefile` copy.** Rejected. It is macOS
only. It inherits the mtime hazard whenever the source target is newer than
the checkout. And `du` counts clones in full, so the Disk Usage page and the
freed-bytes reports would lie.

**Delete `target/` at the end of every turn.** Rejected. A session teardown
never reclaims disk (ADR 0035), and most turns are followed by another within
the hour.

**Only lower the thresholds.** Rejected on its own. A guard that fires only
under pressure still lets idle targets fill the disk up to the threshold, and
it cannot touch live sessions anyway.

## Amendment: hard pressure strips at once, and liveness counts background tasks

Plan: [`docs/plans/2026-09-27-hard-disk-pressure-strips-targets-at-once.md`](../plans/2026-09-27-hard-disk-pressure-strips-targets-at-once.md).

Below `FREE_DISK_HARD_BYTES` the Tier 1 idle window is zero, the same as the
Tier 0 grace. A session that just finished `/harden` or e2e leaves 15 to
25 GB. A burst of them fills the last few GB well inside a 1 h window.

The 1 h window had stood in for liveness. Liveness is now asked directly. A
thread is active when any of three things holds:

- it has a live agent session;
- a coding-agent spawn for it is in flight (`SpawnsInFlight`);
- it has a running background task.

The old session-only probe missed the last two. A spawn sets up the worktree,
`node_modules` hardlinks included, long before its session registers. A
background task builds in the worktree after its session ended the turn. All
three sources live in memory and die with the engine, so the probe never waits
on the database. `EngineActiveThreads` is the one probe, and the Disk Usage
cleanup asks it too.
