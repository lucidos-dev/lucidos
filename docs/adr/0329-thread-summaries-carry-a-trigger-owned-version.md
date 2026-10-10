# 0329: Thread summaries carry a trigger-owned version, and the client never applies an older one

- **Status**: Accepted
- **Date**: 2026-09-29

## Context

A colleague saw "Aborted" flash before "Working" on a coding-agent thread. The
label came from a guess in `exchangeStatus`: the client's copy of the thread
status read idle while the turn's first steps were already arriving.

That copy had no order. Three paths write it: the per-event SSE aggregate, the
thread list and by-id reads, and the events read's `currentAggregate`. The only
guard compared `last_activity` strings.

That guard failed three ways. Several status changes never bump the column.
The client mixed it with engine broadcast time. The engine can also broadcast
two emits on one thread out of commit order. So an older status could land
over a newer one. Plan:
`docs/plans/2026-09-29-thread-status-is-versioned-and-event-backed.md`.

## Decision

`thread_summaries.summary_version` goes up by one on every change to the row.
Two Postgres triggers own it, so no writer can skip or choose it. Every summary
and aggregate the API serves carries it. The client refuses any summary older
than the one it holds, through `applySummaryVersion`, the only writer of
`ThreadMeta.status`.

## Rationale

- **A version orders reads; a timestamp only dates them.** Two reads with the
  same version saw the same row, whatever the clocks say.
- **A trigger covers writers nobody listed.** The projection writes the row from
  dozens of `UPDATE` sites. There are also raw boot-time writes and compose
  PUTs. A trigger bumps for all of them, including code not yet written.
- **The rule is local on the client.** Keep the highest version seen. It needs
  no knowledge of which event types move which field.

## Consequences

- The version bumps on any change, compose drafts included. The client only
  compares it, so a bump the aggregate does not carry costs nothing.
- The schema's first trigger. A reader tracing a write must know the row carries
  one; the migration and `ThreadSummary::summary_version` say so.
- `ThreadMeta.status` and `summaryVersion` are `readonly`. A source scan catches
  casts and `Object.assign`. Tests set status through `applySummaryVersion`.
- The optimistic overlays (`answeringThreadIds`, pending messages) stay: they
  cover the moment before any event exists, which no version can.

## Alternatives considered

- **Version by event sequence.** Tie the status to the sequence of the last
  folded event. It lost because some status writes carry no event (the parent
  wake and its undo), so one sequence could name two statuses.
- **Bump the version in Rust at each `UPDATE`.** Same effect today, and the
  next writer forgets. The trigger makes forgetting impossible.
- **Fix each timestamp guard.** Bump `last_activity` on every status change and
  stop mixing clocks. It lost because it still leaves order to wall clocks, and
  every new status write must remember it.
