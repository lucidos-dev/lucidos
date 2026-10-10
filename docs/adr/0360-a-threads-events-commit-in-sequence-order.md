# 0360: A thread's events commit in sequence order: a per-thread advisory lock is the first lock of every append

- **Status**: Accepted
- **Date**: 2026-10-04

## Context

A client catches up on a thread with `GET /api/v1/threads/:id/events?after=<lastDbSeq>`,
which reads `sequence > after`. The paged cold load hands it `max_sequence` as
its starting mark. Both assume one thing: once a reader sees sequence `s` of a
thread, every lower sequence of that thread is already committed.

`EventBus::emit` did not keep that promise. It draws `sequence` at the INSERT,
runs the projection, and only then commits. Nothing made two emits on one
thread wait for each other before the INSERT. Most projection arms write the
thread's `thread_summaries` row, which serializes them from that write on, but
not before it. About 40 event types write no column at all, `ContextCaptured`
among them, and those never wait for anything.

So a parent's slow `ChildThreadCompleted` (sequence 100) could commit after its
own `ContextCaptured` (sequence 101). A catch-up that read 101 moved the
client's mark past 100, and every later catch-up skipped the completion card.

The report that surfaced this also described commits lagging 13 to 49 seconds
behind their `created` stamp. That part was not real. The dev Postgres runs in a
Docker VM whose clock drifts behind the host's (ADR 0053). Claude Code stamps
each tool call with the host clock. Joined to their `CodingAgentToolCalled` rows
by `tool_use_id`, 8,139 calls showed the database stamping events up to 49
seconds *before* Claude Code produced them. Only a slow database clock does
that.

The plan `docs/plans/2026-10-04-per-thread-event-commit-order.md` holds the
evidence.

## Decision

Every append of an `events` row that carries a `thread_id` first takes that
thread's **thread append lock**: `pg_advisory_xact_lock` on a fixed class and
`hashtext(thread_id)`. In `emit` it is the `Serialize` phase, the first
statement of the transaction, ahead of Validate. `replay_historical_event` takes
it too. `persist` stamps `created` with `clock_timestamp()`.

## Rationale

The lock is held from before the INSERT until commit. A later emit on the same
thread draws its sequence only once the earlier one is visible. So per-thread
commit order equals per-thread sequence order.

It must be the **first** lock its transaction takes. A transaction waiting for
it then holds nothing else, so it can never close a deadlock cycle. Placed
after Validate it would deadlock: Validate's `child_was_moved_out` locks a child
row, and a same-thread emit holding the append lock reaches that row in
`settle_parent_callback`.

`clock_timestamp()` replaces `NOW()` because `NOW()` is the transaction start,
which comes before the lock wait. Under contention a transaction that began
first can take the lock second, and `NOW()` would then stamp it earlier than an
event it follows. The paged read orders by `(created, sequence)`, so the two
orders have to agree.

## Consequences

- Emits on one thread now serialize for their whole transaction. Most already
  did, from their first summary-row write on. The projection-free ones now wait
  too, and they are cheap.
- One extra round trip per persisted thread emit.
- A hash collision between two thread ids only makes those two threads wait for
  each other. It cannot deadlock, because the lock is still each
  transaction's first.
- SSE broadcast order is untouched. The lock is released at commit, before
  `event_tx.send`, so two emits can still broadcast in the reverse of their
  commit order. The client never moves `lastDbSeq` from an SSE frame, so no
  read skips an event.
- The guarantee is per thread. A reader that keeps a **global** sequence mark,
  as the event-wait watermark does, is not covered by it.

## Alternatives considered

**`SELECT … FOR UPDATE` on the thread's `thread_summaries` row.** Rejected. The
row does not exist before a thread's first event, so the first emits would not
serialize. It would also add a row lock to the order that ancestor rollup and
the moved-child check rely on.

**A client-side overlap: catch up from `lastDbSeq - k`, deduplicating by id.**
Rejected. No `k` is safe, because a commit can lag by any amount. It also moves
a server ordering bug into every client.

**One global lock, so commit order equals sequence order for the whole store.**
Rejected for this problem. It would serialize every emit in the workspace, and
one slow projection would then stall all of them. The per-thread reader needs
only the per-thread guarantee.

**Leave `created` as `NOW()`.** Rejected: see Rationale. Nothing reads
`created = now()` against an events row, so the change is safe.
