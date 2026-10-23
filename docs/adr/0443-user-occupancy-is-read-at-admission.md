# 0443: User occupancy is read at admission

- **Status**: Accepted (amends ADR 0010; does not change ADR 0008's policy)
- **Date**: 2026-10-12

## Context

ADR 0010 made the user half of the Thread Queue pool a mirror of
`thread_summaries.status`. The gate seeded a slot, and one broadcast subscriber
called `reconcile_user_slot` on every status-changing event.

The subscriber skipped on `RecvError::Lagged`. A lag therefore dropped the
reconcile for every event it overran. A resumed thread then ran with no slot,
and the pool admitted past its limit. A thread that idled kept its slot until
the gate's guard dropped, which for a parked coding-agent thread is never.

The mirror was a second copy of one fact. Its only link to the first copy was
a channel allowed to lose messages. Audit item 7 in
`docs/plans/2026-10-12-claim-at-entry-instead-of-check.md` traced this.

## Decision

Admission reads user occupancy itself, under the queue's state lock:

- every `running` user-initiated thread in `thread_summaries`,
- any `running` thread a live gate reservation is for, whoever started it, and
- each gate reservation not yet seen: its thread has not started a turn since
  admission.

The subscriber stays, as a drain hint. On a status-moving event of a user
thread, or of a thread a live reservation is for, it drains and refreshes the
panel. On a lag it does the same.

## Rationale

- **One copy of the fact.** `thread_summaries.status` is already correct on
  every path. ADR 0010 derived the pool from it but kept a cache in between.
  Reading it at the decision removes the cache, and with it the lag window.
- **The gate still reserves before the projection flips.** ADR 0010 rejected
  "status alone" because back-pressure needs a synchronous claim. The
  reservation is that claim. It lives from admission until its thread starts
  running, and from then on the status term counts the thread.
- **"Seen" is read from the event log, not observed.** A thread can start and
  park between two admissions, so it is never observed `running`. A
  reservation records its thread's highest event sequence at admission. It is
  seen once the thread has a turn-start event above that sequence. A parked
  thread therefore holds nothing, guard or no guard, which is ADR 0010's core
  property.
- **A lag costs a drain, never a count.** The hint only decides *when* waiting
  work is re-examined. A missed hint delays a drain until the next completion,
  release or lagged receive.

## Consequences

- Every admission and drain does one indexed query for running user threads
  (`idx_thread_summaries_running_user`), plus one for unseen reservations when
  any exist. Both sets are small: one row per running user thread, and one
  reservation per chat task in flight.
- `reconcile_user_slot`, `parked_user_slots` and the re-file-under-the-parked-key
  logic are deleted. A reservation stays until its guard drops, marked seen
  once its turn starts. The guard's drop removes it, or a waiter.
- A user follow-up into an agent-started thread counts while it runs. The row
  keeps `initiator = 'system'`, so only the live reservation puts the thread in
  the read. ADR 0010's reconcile dropped such a slot on the first status event.
- A follow-up whose message already set its thread `running` is admitted
  against the pool minus its own thread, so it never waits for itself.
- Every reservation names its thread. A chat request with no thread id gets
  one minted at the gate, as a client's raw new send already carries. So
  `acquire_user_slot` takes a `Uuid`, and a reservation can always be seen.
  ADR 0010's mirror counted such a request twice while its thread ran.
- A failed occupancy read admits: it counts no running threads and marks no
  reservation seen. Blocking every chat on a transient query error is the worse
  failure.

## Alternatives considered

- **Keep the mirror, feed it a directed channel.** An unbounded channel from
  the bus would deliver every status event, so the mirror would not drift on
  lag. Rejected: it keeps two copies of one fact. Any status write that
  bypasses the bus (the boot settle's direct `idle` write) still
  desynchronizes it.
- **Mark a reservation seen when an admission observes its thread running.**
  Rejected: a thread that starts and parks between two admissions is never
  observed, and its reservation would hold a slot while it waits on the user.
- **Compare `last_revived_at` against an admission timestamp.** Rejected: the
  projection stamps `NOW()`, the transaction's start time. So a commit can land
  a stamp older than an admission that preceded it. The event sequence is
  allocated in order, so it has no such window.
- **Delete the gate and derive back-pressure from status alone.** Still
  rejected, for ADR 0010's reason: the projection flips after the gate must
  decide.
