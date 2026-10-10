# 0364: An event-wait watermark is a committed horizon: appends advertise an in-flight lock and the watermark read waits them out

- **Status**: Accepted
- **Date**: 2026-10-04

## Context

An *event wait* records a watermark and has two delivery paths. The live
dispatcher offers each broadcast to the waits in its cache. The catch-up scan
reads `sequence > watermark`. Registration runs the scan after it emits
`EventWaitStarted` and inserts the wait into the cache. The boot rebuild and
the Lagged re-scan run it from the persisted watermark.

The watermark was `MAX(sequence)`. `EventBus::emit` draws `sequence` at its
INSERT and commits later, so a lower sequence can commit after a higher one.
Event E holds 100 uncommitted, 101 commits, and the watermark reads 101.

E then commits, and its broadcast reaches a cache that does not hold the wait
yet. The scan from 101 never sees E either. The thread sleeps until its
deadline, silently. The engine-armed background-task wait is the most exposed,
since a task can finish just as the wait arms.

Repairing the live race alone does not fix it. Say the engine crashes after E
commits and before the dispatcher delivers it. The boot rebuild scans from the
stored 101 and misses E again. The persisted number itself is wrong.

ADR 0360 makes a *thread's* commit order equal its sequence order. That
guarantee is per thread and does not cover a global mark.

The plan `docs/plans/2026-10-04-event-wait-watermark-is-a-committed-horizon.md`
holds the reconnaissance.

## Decision

A watermark is a **committed horizon**: a sequence `H` such that every events
row with `sequence <= H` has committed, or never will.

Every transaction that appends an events row takes an **in-flight append lock**
as its first statement: `pg_advisory_xact_lock(class, pg_backend_pid())`.
`EventBus::begin_append` opens the persisted thread arm, the persisted system
arm and `replay_historical_event` this way.

`committed_event_horizon` reads `S = MAX(sequence)`. Then it lists the granted
in-flight locks from `pg_locks`, scoped to `current_database()`. It takes each
one in shared mode, which waits until that transaction ends, and releases it at
once. Then it returns `S`. The whole wait shares one 10 s deadline, enforced
through `lock_timeout`, and running out is an error.

## Rationale

**Why the read is a committed horizon.** Take a row `r` with `sequence <= S`
that is uncommitted when the read returns.

1. `r` drew its sequence no later than the row holding `S`.
2. That row committed before the `MAX` read, so `r`'s append was in flight at
   that read.
3. `r`'s transaction took its in-flight lock before drawing a sequence, so the
   lock was granted before the `MAX` read.
4. The lock list is read after the `MAX` read, so it contains `r`'s key.
5. The read waits for that key's transaction to end, so `r` committed or rolled
   back before the read returned.

So no such `r` exists.

**Why that fixes every path.** Any event that commits after the read returns
holds a sequence above `S`. If it commits before the cache insert, the scan
that follows the insert sees it. If it commits after, its broadcast follows the
insert, so the dispatcher sees the wait. After a crash, it was either delivered
or sits above the persisted watermark, where the boot scan finds it. The scans
themselves do not change.

**Why the key is the backend pid.** A backend runs one transaction at a time,
so the key is unique among concurrent appends. Appends never wait on each
other. A reader holds a backend's key only between grant and release, so that
backend's next append waits for an instant at most.

**Why no deadlock.** The in-flight lock is each append transaction's first
lock, so a transaction waiting for it holds nothing. The reader holds no lock
while it waits. The `MAX` and `pg_locks` reads run outside its transaction, and
it releases each key the moment it is granted. Nobody waits on the reader, so
no cycle can pass through it, not even one running through application code.

ADR 0360 adds the thread append lock, which also claims to be first. With both,
the in-flight lock comes first and the thread append lock second. A transaction
waiting for the thread append lock then holds only its own in-flight key. Only
a reader waits on that key, and a reader closes no cycle, so ADR 0360's argument
still holds.

**Why a timeout, and why an error.** An append takes milliseconds. One in
flight for 10 s is stuck on something else, and blocking an `await_event` call
behind it helps nobody. A guessed horizon would bring the silent stall back. An
error goes through each caller's existing loud path instead. Registration
refuses and the model retries; the background-task arm logs that the work will
finish unwatched.

## Consequences

- One extra round trip per persisted emit and per replay. The thread arm now
  takes two lock statements, which could fold into one `SELECT`.
- Arming a wait may wait out the appends in flight when it began, normally for
  milliseconds.
- An event that commits during that wait sits at or below the watermark and is
  visible. The arming lookback reports it, which is right for a pre-arm event.
- Waits armed before this change keep their `MAX(sequence)` watermark until
  they resolve, within 24 hours. No backfill.
- Any other reader that keeps a global sequence mark can call
  `committed_event_horizon` and inherit the guarantee.
- A writer that inserts events rows without `begin_append` silently breaks the
  guarantee. `EventBus` is the only sanctioned writer (`.claude/rules/rust.md`),
  and the tests in `event_bus_tests/committed_horizon.rs` cover all three of
  its append paths.

## Alternatives considered

**A global barrier.** Every append takes one shared advisory lock, and the
watermark read takes it exclusively. Rejected. A waiting exclusive request
queues every new shared request behind it, so one append stuck on a row lock
stalls every append in the workspace. A non-queueing `pg_try_advisory_lock`
loop avoids that but can starve under steady traffic.

**Advertise the reserved sequence.** Each append draws `nextval` first, then
takes an advisory lock keyed by it, and the watermark becomes the lowest
advertised sequence minus one. Rejected. Between drawing and advertising, a
reader sees neither the row nor the lock. Closing that gap needs the advertising
to happen first, which is this decision. A "lowest minus one" mark is also
wrong in the other direction: the scan would deliver events already committed
before the wait existed.

**Make the wait live before reading the watermark.** Insert a pending wait into
the cache, buffer any live match until `EventWaitStarted` persists, then read
the watermark. Rejected as incomplete. It closes the in-memory race, but the
persisted watermark stays a bare `MAX(sequence)`. A crash between the late
commit and its delivery reopens the stall at boot. It also adds a pending state
to the cache.

**Record a transaction snapshot with the watermark.** Store
`pg_current_snapshot()` and make the scan include rows whose writing transaction
was invisible to it. Rejected. It needs a new `xid8` column on `events` with its
own index, and the transaction id must exist before the sequence is drawn. That
ordering needs an extra statement per append anyway, so it costs more than this
and adds a migration.

**Commit timestamps.** Scan by commit time rather than sequence. Rejected. It
needs `track_commit_timestamp`, a server setting the engine does not control on
every Postgres it runs against.
