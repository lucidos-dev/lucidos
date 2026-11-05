# 0453: A lagged bus subscriber re-reads its state or replays the dropped rows; a fact never gets a directed channel

- **Status**: Accepted
- **Date**: 2026-10-12

## Context

The `EventBus` broadcast holds `BUS_CAPACITY` frames. A subscriber that falls
further behind gets `RecvError::Lagged`, and the frames it missed are gone from
the channel. Nine subscribers logged the lag and carried on, so a burst lost
work with no trace:

- a memory index entry;
- an archive's set-aside branch work, or a canceled wait's held proposal;
- a model, `vertex_region` or provider change, until restart;
- a change summary;
- a todo settle;
- a background-task kill;
- the answer a voice caller was waiting for.

The parent plan's invariant I12 says a broadcast lag never drops work. Its
Change D gave spawn requests a directed channel (ADR 0443). The plan
`docs/plans/2026-10-12-a-lagged-subscriber-replays-what-it-missed.md` holds the
reconnaissance for the rest. The plan
`docs/plans/2026-10-13-the-bus-names-a-subscriber-s-replay-start.md` closed the
start-up windows the first version left.

## Decision

A lagged subscriber repairs from the database in one of two shapes:

- **State refresh.** A subscriber that mirrors state re-reads that state: the
  `models` table, the stored preference, the provider build.
- **Lag replay.** A subscriber that works per event reads back exactly the rows
  it missed, through `engine::event_bus::LagReplay`, and runs its ordinary
  handler on them.

None of these gets a directed channel.

## Rationale

**The events table already holds every fact.** A lag loses the frame, never
the event, so the repair reads what the bus wrote. That is the bus contract's
own answer, and five subscribers already worked this way.

**A directed channel makes the producer name its consumers.** That breaks
broadcast and subscribe. Spawn requests earned the exception because each is a
command with one consumer and an idempotency key. A fact has many readers and
no single owner.

**The replay window is exact.** Its top is a committed horizon (ADR 0364), so
a row that committed out of sequence order is never skipped. A thread's own
window uses its `MAX(sequence)`, which ADR 0360 makes a horizon.

**A replay never acts twice.** It skips rows the subscriber acted on live, and
a live frame it already returned is skipped in turn. A memory extraction is a
billed call, and a voice answer said twice talks over the caller.

**The bus names the start position; no read does.** The bus commits a row,
then broadcasts it. A row can commit before any floor read and still broadcast
after the subscription opens. A lag that drops that frame loses it, whichever
side of `subscribe()` the read sits on. Reading after subscribing leaves that
window. Reading first widens it, and drops every frame emitted during the read.

So the bus keeps an *append ticket* for each persisted append, from
`begin_append` until it ends. Each send records itself under one lock.
`LagReplay::subscribe` takes the receiver and the position under that lock:

- `M`, the highest sequence an append has learned. Every row sent before the
  subscription is at or below it.
- the appends still in flight. Only these can still send a row at or below
  `M`, because a later append draws a higher sequence.

The subscriber owes the rows above `M` and the rows those appends send after
it subscribed. Subscribe stays synchronous at every spawn point, and boot waits
on no read.

**Replay where state would guess.** The background-task reaper replays the
`ThreadArchived` and `ThreadDiscarded` it missed rather than reading
`archive_state`. An unattended trigger thread is archived by its terminator with
no `ThreadArchived`, while its background task still runs. The live path leaves
that task alone, and a state read would have killed it.

**A failed read moves nothing.** The floor stays, so the next lag covers both
windows. Nothing re-runs on its own: no retry, timer or sweeper.

**There is no start read to fail.** If the bus has learned no sequence when a
subscription opens, it has sent nothing, so the subscriber owes every row. Its
first lag reads the window's top `H`. Every row the bus appended at or below
`H` has committed, so it was learned. The floor is then the lowest learned
sequence, less one.

## Consequences

- `SystemEvent` derives `Deserialize`, so a stored system row reads back as its
  event. `ChangesUpdated` is broadcast only, so its change lists skip it.
- A replayed frame carries no aggregate snapshot, because none is stored. Its
  chain depth and emitting trigger come from the row (ADR 0137).
- Each replaying subscriber keeps the sequences that reached it live above its
  floor, as ranges. The set starts empty with each engine.
- Each persisted append takes one in-process lock, shared by every append, to
  open, learn, send and end. It is held for a map update or one channel send,
  never across an await. A transient send takes none.
- A replay offers an in-flight append's row only once it was sent. A row still
  unsent arrives live, or a later lag owes it.
- The event-wait dispatcher's match delivery needs none of this. Each wait
  rescans from its own persisted watermark, which never moves (ADR 0364). Its
  lag arm replays dropped `ThreadArchived` and `ThreadDiscarded` rows instead,
  before the rescan. A wait whose thread was archived is then gone before the
  rescan can match it.
- Rows another process appends never reach this bus live. A replay can return
  one above the floor.
- The scheduler's lag arm replays through the same `LagReplay`. It pages a
  `ReplayWindow` itself, and the row stores the emitting trigger it needs.

## Alternatives considered

- **A directed channel per consumer.** Rejected for the coupling above, and
  because it is lost on a crash exactly as the broadcast is.
- **A full rebuild on every lag.** The memory rebuild re-extracts every event
  that once yielded nothing, which bills the whole history again.
- **Raising `BUS_CAPACITY`.** It makes a lag rarer, never impossible.
- **A plain `MAX(sequence)` watermark.** A row can commit below it after the
  read, and the scan then never sees it (ADR 0364).
- **A committed horizon read before subscribing, then a catch-up replay.** A
  row that commits before the read and broadcasts after the subscription is
  still lost. Boot would also wait on one read per subscriber.
