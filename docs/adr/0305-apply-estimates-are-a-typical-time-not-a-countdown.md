# 0305: Apply estimates are a recency-weighted typical time from event history, never a countdown

- **Status**: Accepted (shown in the Lucidos menu rather than a toast since 0306)
- **Date**: 2026-09-27

## Context

An Apply All batch sat on "Applying thread 1 of 4 · Resolving merge conflict"
with an empty bar for many minutes. A conflict resolution and a hardening each
run a whole coding-agent session, and the engine reports no progress inside
one. The user asked for an estimate computed by the engine, weighted toward
recent runs because the process keeps getting faster. They also asked for a
pre-check of which changes will conflict.

In the dev workspace, a successful conflict resolution took a median of 18
minutes and a 90th percentile of 40. A hardening took a median of 20 and a 90th
percentile of 68.

## Decision

The engine derives a typical duration per slow phase from the `events` table:
the recency-weighted median of completed runs. The toast shows it beside the
elapsed time ("6 min, usually ~18 min") and never counts down. A
`git merge-tree` probe predicts, per pending change, whether its merge would
conflict.

## Rationale

- **A typical time, not a countdown.** The spread is two to three times the
  median. A countdown would reach zero on most slow runs and then read as a
  stall, which is the complaint this answers.
- **Recency weighting.** A run's weight halves every 14 days over a 90-day
  window. Old slow runs then give way to new fast ones, without a cliff at the
  window's edge.
- **Only successful runs count.** A conflict run ends at `ChangeApplied`, a
  hardening at `ChangeHardened`. A failed run measures an abort, and a run past
  four hours measures a user who walked away.
- **A floor of five runs.** Fewer give noise, so the toast shows elapsed time
  alone.
- **History, not a new table.** Every start and end is already an event. A
  cache on the engine keeps the 0.6 s query off the broadcast path.
- **`merge-tree`, three-way.** It computes the merge without touching a ref, the
  index or a worktree. A git failure is `unknown`, never `clean`, per the
  unknown-git-state rule in `.claude/rules/rust.md`.

## Consequences

- Estimates follow the workspace's own pace, and a new workspace shows elapsed
  time only until it has five runs of a phase.
- A prediction is against `main` as it stands. In a batch, an earlier member
  landing can create or remove a later member's conflict. The prediction
  refreshes on the next changes broadcast, when `main` has moved.
- A plain merge gets no estimate. It takes seconds and no event marks its start.

## Alternatives considered

- **A countdown or ETA clock.** Rejected for the spread above.
- **The plain median of a fixed window.** Rejected: it lags an improving
  process by the whole window, which the user named as the reason to weight.
- **A progress fraction inside a phase.** No signal exists: a coding-agent
  session does not know how far through a merge it is.
- **A new durations table fed at each phase end.** Rejected: it duplicates what
  the events already record, and needs a migration and a backfill.
