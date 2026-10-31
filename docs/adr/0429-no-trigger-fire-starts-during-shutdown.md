# 0429: No trigger fire starts once engine shutdown has begun

- **Status**: Accepted
- **Amended by**: [0579](0579-restart-cut-trigger-fire-fails-visibly-and-retries-once.md)
- **Date**: 2026-10-11

## Context

An Apply restarted the engine. The old engine began its teardown, and its
event path correctly refused to fire triggers. Thirteen seconds later its
cron path started a script trigger anyway. The script's `lucidos data write`
met a closed API listener and failed, so a failure push went out. The new
engine's missed-slot catch-up then saw a recorded run for the slot and
skipped it, so the work never happened.

The scheduler kept a private shutdown flag. `main.rs` set it last, after the
API had closed. The event path read the engine's own signal, which
`begin_teardown` sets first. The catch-up read neither. The Thread Queue had no
shutdown check at all. The teardown frees pool slots by stopping coding
agents, so a trigger fire queued before shutdown started too.

## Decision

The engine's `shutting_down` atomic is the one shutdown signal. From the
moment it is set, no trigger fire starts:

- Event dispatch drops fires from shutdown-time events, as before.
- Both cron paths, on-time and catch-up, ask one `scheduled_fire_refusal`
  before they submit. A refused slot records nothing.
- The Thread Queue holds a clone of the same atomic and admits no `cron` or
  `event-trigger` entry once it is set. The entry stays `queued`, and the
  next engine's boot recovery runs it.

The catch-up rule is unchanged. A cron slot counts as run when its trigger has
a `TriggerExecuted` at or after the slot, whatever its status. A refused slot
has none, so the next engine fires it within the 60-minute grace.

## Rationale

Refusing before the start is what makes the existing rule true. The event
store then holds no run for a slot that never ran. A slot that ran, `ok` or
`failed`, keeps its row and is not repeated. The catch-up needs no
shutdown-window arithmetic and no second notion of "ran".

A queued fire already has an owner that survives a restart: the persisted
`thread_queue` row. Holding it there reuses boot recovery rather than adding a
path. A catch-up of the same slot coalesces with the held cron entry, so the
two cannot double.

## Consequences

- A cron slot that comes due during a restart runs once, on the next engine,
  if that engine starts within the grace window.
- A run already in flight when shutdown begins finishes as before. If it fails
  because the API went away, it stays failed and is not re-run.
- `SchedulerManager::shutdown` marks the engine shutting down, so it still
  stops every runner when called alone.

## Alternatives considered

- **Re-run a failed slot whose run fell in the shutdown window.** Reading a
  window back from the event store needs timestamps for when each teardown
  began and ended, which nothing records. It would also re-run a genuine
  failure that happened to coincide with a restart.
- **Record an `interrupted` run status for a run cut by shutdown, and let the
  catch-up skip it.** A new wire value, panel label, replay arm and catch-up
  filter. Worse, an intent trigger's thread is auto-resumed by thread
  recovery after a user switch, so a catch-up re-fire would run it twice.
- **Defer the cron fire through the Thread Queue instead of refusing it.** It
  would run on the next engine however late that engine started, hours after
  the slot. The grace window is the designed policy for a missed slot, and an
  engine that is shutting down has missed it.
- **Keep the scheduler flag and set it earlier.** Two copies of one signal is
  how the cron path came to read the late one.
