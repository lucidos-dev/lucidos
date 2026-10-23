# 0529: The cache keep-alive shows one status per waiting thread; its pings stay invisible

- **Status**: Accepted (the divider is amended by [0540](0540-a-keep-alive-run-leaves-one-card.md))
- **Date**: 2026-10-19
- **Amends**: [0470](0470-keep-the-prompt-cache-warm.md). Its "the turn leaves
  no visible trace" still holds for each ping. The keep-alive as a whole is
  now visible.

## Context

ADR 0470 made the *cache keep-alive* leave no trace. That kept the transcript
clean, but it also hid real spend. A waiting thread could send 30 pings and the
user saw nothing: not that the engine kept the cache warm, not what it cost,
and not when it gave up. They also could not tell why a thread waiting on a
`no-wake` sub-thread got no keep-alive at all.

## Decision

The engine shows one aggregate *cache keep-alive status* per waiting thread,
and never the ping turns themselves.

- The keep-alive tick is its one writer. It emits `CacheKeepAliveStatusChanged`
  only when the status changed, and the event projects into
  `thread_summaries.cache_keep_alive_status`.
- The status is `warming`, `stopped` (past break-even) or `not_kept` (with a
  reason). It is absent when there is nothing to keep warm.
- The client draws it twice: a dashed divider at the end of the transcript,
  and a "Prompt cache" section in the waiting panel. Both read the thread meta
  signal. Neither polls.
- Each ping turn still adds nothing to the transcript and moves no recency.
- Token Cost labels the ping calls "Cache keep-alive", told apart by the
  keep-alive anchor their `request_event_id` names.

## Rationale

- **Spend the user cannot see is spend they cannot judge.** One line with the
  ping count, the cost and the next ping says what the engine does with their
  money, without a card per ping.
- **One writer keeps the status honest.** The tick already holds every input:
  the wait start, the observed TTL and the model's prices. A pure `status()`
  beside `decide()` shares its arithmetic, so the panel cannot say "kept warm"
  after the engine stopped.
- **An event, not a side table.** `thread_summaries` is a projection, so every
  column must derive from an event (`core/announced_surfaces.rs`). The event
  also rides SSE and survives a restart.
- **The event is `Metadata`.** It moves no section, status or recency, so a
  status change never wakes or reorders a thread.

## Consequences

- The events table gains one row per status change: about one per ping, since
  each ping moves its count and cost, never one per 10 s tick.
- The status is live only. Once the wait ends, it clears, and Token Cost holds
  the history.
- Codex threads show nothing, since they get no keep-alive.

## Alternatives considered

- **Show each ping as a transcript entry.** Rejected: 30 identical entries
  bury the real conversation, and ADR 0470's reason for hiding them still holds.
- **Derive the status on the client.** Rejected: the client lacks the TTL,
  the budget and the model's prices, and a copy of `decide()` would drift.
- **A purpose of its own for keep-alive captures.** Rejected: every non-turn
  `ContextPurpose` pairs with an auxiliary producer, which would move the pings
  into the context meter and the transcript's auxiliary rows. The anchor link
  identifies them without that, old rows included.
- **A status row written outside the event bus.** Rejected: it would announce
  nothing, so neither SSE nor a restart would carry it.
