# 0554: A per-thread switch turns the cache keep-alive off until the thread runs again; amends 0529 and 0547

- **Status**: Accepted
- **Date**: 2026-10-23
- **Amends**: [0547](0547-keep-alive-beats-a-parked-session-by-copy.md).
  `cache_keep_alive_mode` stays the workspace rule. A thread can now opt out of
  it for one wait.
- **Amends**: [0529](0529-keep-alive-shows-one-status.md). The status
  gains a `turned_off` reason.

## Context

The keep-alive gives up at break-even, or at half of it on a card. That rule
is right when nobody knows how long the wait will be. Sometimes the user does
know. They see a question card, know they will answer tomorrow, and every ping
until the give-up is money they would rather keep. Today the only way out is
the workspace setting, which turns the keep-alive off for every thread.

## Decision

- **A "Turn off for this thread" control** sits in the "Cache kept warm"
  popover, beside the setting link. The waiting panel's Prompt cache section
  carries the same control as a link. It shows on every thread kind the
  keep-alive covers.
- **It is an event.** `CacheKeepAliveTurnedOff` and `CacheKeepAliveTurnedOn`
  are thread events with the device actor, classed `Metadata`. They project
  into `thread_summaries.cache_keep_alive_off_at`: set by the first, cleared
  by the second. The tick, the status and the cache-warm line read only that
  column.
- **It holds until the thread runs again.** The switch is in force while
  `cache_keep_alive_off_at` is later than the thread's last real request. A
  real request is a turn capture that no keep-alive anchor started. The next
  turn, or the answer to the card, re-arms the keep-alive with no event.
- **It stops the run at once.** The next tick, within 10 s, sends no beat and
  writes the status `not_kept` with reason `turned_off`. A beat already in
  flight finishes: a copy is capped at 60 s, and a prefill takes seconds.
- **It is per thread, never inherited.** Turning a parent off stops the
  parent's beats, even while its sub-threads work. Turning a sub-thread off
  stops that sub-thread's beats only. Whether a working sub-thread keeps its
  parent warm does not change.
- **"Turn back on"** shows while the switch is in force and the cache is still
  warm. It emits `CacheKeepAliveTurnedOn`, and the next tick beats again if the
  wait still qualifies.
- **The popover says what the reply will cost.** Once off, it names the time,
  the moment the cache goes cold, and the reread in tokens and dollars.
- **Prompt-first.** The Lucidos Agent's `threads` tool gains the same switch,
  so a user can ask for it in words.

## Rationale

- **An event, because it is a user decision.** The projection survives a
  restart, rides SSE, and keeps one writer. ADR 0529 requires every
  `thread_summaries` column to derive from an event.
- **One wait, not the thread's life.** The user judges this wait: "I will
  answer later." The next wait has its own odds. A switch that stayed off would
  silently pay full re-writes on later sub-thread reports, long after the user
  forgot it. The workspace setting already serves "never for me".
- **Re-arming needs no event.** It falls out of a comparison with the last
  real request, so no second writer and no timer can miss it.
- **Not inherited, because each wait has its own economics.** A parent waiting
  on a sub-thread that waits on the user is already not kept: the existing
  `children_awaiting_user` rule covers that case. Cascading would hide
  effects in threads the user did not touch.
- **Subagents are out of reach anyway.** ADR 0511 counts a Claude Code
  subagent as a blocking call inside a turn. No keep-alive beats there, so the
  switch has nothing to stop.

## Consequences

- One migration adds `thread_summaries.cache_keep_alive_off_at`.
- `decide()` gains `Skip(TurnedOff)`, and `status()` gains
  `NotKept{TurnedOff}`, for every route.
- The cache-warm response gains `reread_usd`, priced from the model catalog.
- A new real-request SQL fragment sits beside `last_turn_started_sql`, which
  still counts resume beats for the keep-alive's own timing.
- The workspace-data estimator still counts these waits: a wait's length does
  not depend on the beats.

## Alternatives considered

- **Off for the thread's life.** Rejected for the reason above.
- **A snooze for N hours.** Rejected: the user knows the wait, not the clock,
  and a timer adds a second writer.
- **A per-thread give-up time.** Rejected: one more number to judge. Off is the
  decision the use case needs.
- **Inherit the switch down the sub-thread tree.** Rejected: hidden effects,
  and the parked-child rule already covers the case.
- **A preference instead of an event.** Rejected: preferences are global or
  per device, and this is per thread and per wait.
