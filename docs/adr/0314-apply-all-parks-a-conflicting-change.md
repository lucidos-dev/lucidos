# 0314: Apply All parks a change that hits a merge conflict and keeps applying the rest

- **Status**: Accepted
- **Date**: 2026-09-28

## Context

An Apply All batch sat on "Change 7 of 9 · Resolving merge conflict" for
twenty minutes, with two changes queued behind it. The batch was strictly
serial: the earliest unresolved member was "in flight", and a conflict kept
its member unresolved for the whole resolver run. The user asked for the queue
to keep going while a conflict resolves, so more changes land sooner.

A resolver already works on its own. `apply_change` hands the conflict to a
coding-agent session and returns `Conflict` at once, and the session lands the
change later. Resolvers on different changes already ran side by side for
single applies. The only global lock is `MERGE_MUTEX`, held for the short step
that moves `main`.

## Decision

A batch member whose apply returns `Conflict` moves to `Resolving`, and the
driver starts the next queued member. Hardening and plain merges stay one at a
time. A `Resolving` member that then fails because a later member conflicts
with it goes back in the queue once.

## Rationale

- **Only the wait is removed.** A resolution needs no driver attention, so
  holding the queue behind it bought nothing.
- **Hardening stays serial.** Two hardening runs at once double the memory and
  test load for no gain the user asked for.
- **Member state is explicit.** `Queued`, `Applying`, `Resolving`, `Applied`,
  `Failed`. "The earliest unresolved member" stopped meaning "in flight" once a
  member can wait off to the side. One rule starts work: when nothing is
  `Applying`, start the earliest `Queued` member.
- **One retry, for one cause.** Keeping the queue moving makes it likelier that
  a later member lands a conflicting edit while a resolution runs. The resolver
  then fails on catch-up. A retry re-applies against the new `main`. It opens a
  fresh resolution, and a Tier-2 or Tier-1 retry keeps the earlier resolution
  on the branch, so the new one is small. Any other failure is final, and so
  is a second collision, so a batch cannot loop.
- **Tier 1 had to learn that `main` moves.** Its landing check asked whether
  `main` now was an ancestor of the branch. Any change landing during the
  resolution failed it, even a clean one. It now asks about the `main` the
  resolution started from, and the catch-up merges the rest.

## Consequences

- A batch with conflicts finishes sooner. Its members can land out of order:
  a parked member lands whenever its resolver does.
- The Lucidos menu shows the member being applied on the Apply All row, and
  each parked member on its own "Resolving merge conflict" row.
- The snapshot in `GET /api/v1/changes` names the applying and the resolving
  members. Recovery rebuilds both from running sessions and conflict pairings,
  so nothing new is persisted. The one-retry allowance is in memory too, so
  an engine restart grants a member one more retry. A batch still cannot
  loop without restarts, and persisting a flag for it was not worth a table.
- Cancel interrupts parked resolvers too.

## Alternatives considered

- **Run every member in parallel.** Rejected: parallel hardening costs the
  memory and the test isolation, and parallel merges only queue on
  `MERGE_MUTEX` anyway.
- **Skip a conflicting member and retry it at the end.** Rejected: the
  resolution would start late and serially, which is the wait this removes.
- **Park without a retry.** Rejected: a collision the batch itself caused would
  fail a change that one more pass would have landed.
