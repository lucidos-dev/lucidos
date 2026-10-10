# 0302: The disk monitor probes, decides and alerts without waiting on the database; a stalled emit is dropped and retried fresh

- **Status**: Accepted
- **Date**: 2026-09-27

## Context

The "Low disk space on your machine" notification fired 12 minutes late and
said 12.0 GB free when about 232 GB was free. The worktree cleanup cycle read
free disk once, at its start. It then waited on Postgres for each worktree, and
Postgres had stalled because the disk was full. When Postgres came back, the
cycle finished and emitted the alert with the old reading. The same stale
reading also opened the retention gate, so Tier 0 reclaimed non-archived
threads' worktrees with 232 GB free.

The general shape: a full disk is exactly what stalls the database. A
free-disk monitor that waits on the database goes silent when the user needs
it most. Then it reports a number that is no longer true.

Plan: [`docs/plans/2026-09-27-low-disk-alert-fresh-reading.md`](../plans/2026-09-27-low-disk-alert-fresh-reading.md).

## Decision

The low-disk alert belongs to a separate task, the *disk monitor*
(`DiskMonitor` in `engine/worktree_cleanup_disk_monitor.rs`). It runs no query.
It probes every 60 s, re-probes right before it emits, and bounds the emit to
10 s. A stalled emit is dropped, the alert stays armed, and the next tick
retries with a fresh reading.

The cleanup cycle keeps its queries unbounded, but reads free disk again at
every pressure decision, after the queries that decision depends on.

## Rationale

- **Split, not timeouts.** Every database-backed tier check already fails safe:
  an unanswered probe skips the worktree. A timeout on those queries would turn
  "wait, then act on current facts" into "skip everything", and the alert would
  still wait behind them. The harm came from acting on a reading taken before
  the wait. Reading at the decision removes it for the gate and the hard
  response, and the split removes it for the alert.
- **Drop and retry, never deliver late.** An emit left in flight completes
  whenever the database returns, with the number it was built with. That is
  the incident again. Dropping the future after the bound means every
  recorded alert carries a reading at most one emit bound old.
- **Degraded behaviour is honest silence.** `NotificationCreated` is persisted,
  and SSE, push and the notification list all derive from the persisted row.
  While Postgres is stalled, nothing can reach the user through them. The
  monitor keeps deciding and logs each failed attempt with its fresh reading.
  The alert goes out on the first tick after the database answers, if disk is
  still low then.
- **A 60 s tick needs a re-arm band.** Re-arming exactly at 20 GB would ping
  the user each time free space jitters across the line. The alert re-arms at
  soft + 2 GB, which keeps "one alert per crossing, re-arm on recovery".

## Consequences

- The alert arrives within about a minute of a crossing, instead of up to one
  15-minute cycle later.
- A timed-out emit that the server committed anyway produces one duplicate
  alert on the retry. We accept that over a late, false number.
- The alert body's footprint is measured from disk alone. It counts orphaned
  `thread-<8hex>` directories that the Disk Usage inventory leaves out, because
  telling them apart needs the database.
- Reclamation still needs the database. During a stall no tier acts, including
  the hard response. It acts on fresh readings as soon as the database answers.
- The alert state stays in memory. A restart re-arms it, which costs at most
  one extra alert.

## Alternatives considered

- **Timeouts on each cleanup query.** Rejected for the reasons above: every
  timed-out check skips its worktree, so a stall turns into a cycle that does
  nothing, slowly. The alert would still sit behind the loop.
- **A timeout around the whole cycle.** Rejected: cancelling a cycle mid-way can
  stop between `git worktree remove` and the branch decision.
- **Spawn the emit and forget it.** Rejected: the spawned emit lands when the
  database returns, with the stale number. That is the bug.
- **Deliver the alert without the database** (a direct push, or a transient SSE
  broadcast). Rejected for now: every delivery path derives from the persisted
  event, and a second path is a new architecture, not a bug fix.
- **Keep the monitor on the 15-minute cleanup cadence.** Offered at plan
  approval and declined: it keeps a detection delay of up to 15 minutes for a
  disk that can fill in less.
