# 0792: The event wait module keeps live waits keyed by wait id as pure copies of start events

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0047](0047-event-wait-is-an-event.md)

## Context

The `event_wait` module keeps a cache of live waits, rebuilds it at boot from
`EventWaitStarted` events, and delivers a woken turn back to its thread. These
comments record the shape choices behind that cache, the stop result, the
re-entry channel, the catch-up scan and the deadline sweep.

This record restates the design comments of
`crates/lucidos-engine/src/engine/event_wait/mod.rs`, written before the
comment strip removes them. Nobody checked it line by line against the code.

## Decision

- `LiveWait` is a copy of its `EventWaitStarted` payload. Nothing derived is
  stored on it.
- The live-waits cache is keyed by `wait_id`, not by thread.
- The arm time is recorded on the event. It is not derived from `expires_at`.
- Stopping a wait returns one of three states, not a bool. A resolved wait and
  a wait whose cancel could not be written are separate outcomes.
- A re-entry travels over the engine's re-entry channel as plain data. A
  consumer task runs the turn. `run_agentic_loop` does not await it in place.
- The catch-up scan filters by subscribed event types in SQL. It evaluates
  conditions in Rust and pages forward until a row passes both.
- The deadline sweep runs every ten seconds over the in-memory map.

## Rationale

- Anything derived and kept on `LiveWait` would be state the boot rebuild has
  to guess at.
- A thread may hold several waits at once, up to `MAX_LIVE_WAITS_PER_THREAD`.
  Delivery consumes exactly one wait, so `wait_id` is the natural key.
- The age of a wait is what makes `list_event_waits` answerable. An age derived
  from `expires_at` would drift the moment a deadline did.
- The two stop failures mean different things to a user. A resolved wait is
  gone and the button was stale. A wait whose cancel failed is still there and
  still cancellable.
- Registration can resolve a wait inline through its catch-up scan. Awaiting
  the turn in place would make the future of `run_agentic_loop` contain itself.
  `WAIT_REENTRY_RX` holds the full argument.
- Evaluating conditions only in Rust gives the condition language exactly one
  implementation.
- A timeout of tens of minutes does not notice ten seconds of slack. The sweep
  is cheap because the map holds a handful of entries per subscribed thread.

## Consequences

- The boot rebuild reads each wait straight from its event, with nothing to
  reconstruct.
- The user sees an accurate message for each failed stop.
- The catch-up scan cannot be a bare `LIMIT 1`. Only Rust can say whether a
  `condition` holds.
- An expiry can land up to ten seconds after its deadline. A wait's
  `timeout_secs` is kept to that resolution.

## Alternatives considered

- Keying the cache by thread. Rejected because a thread may hold several waits
  and delivery consumes one.
- A bool for the stop result. Rejected because it tells the user "already
  resolved" about a wait that is about to keep waiting.
- Deriving the arm time from `expires_at`. Rejected because it drifts when a
  deadline moves.
- Awaiting the re-entry turn in place. Rejected because it makes the
  `run_agentic_loop` future contain itself.
- Evaluating conditions in SQL. Rejected so the condition language keeps one
  implementation.
