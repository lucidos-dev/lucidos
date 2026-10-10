# 0280: The event-wait loop cap counts only waits no other thread ended, at 20 an hour

- **Status**: Accepted. Supersedes the Decision of ADR 0265; its one-hour window stays.
- **Date**: 2026-09-25

## Context

ADR 0265 made the loop cap a rate: ten waits in a rolling hour with no human
message between. Its premise was that a loop re-arms in seconds and a serial
workflow every twenty to sixty minutes.

The same evening the Nightly Build thread disproved it. It waited for seven
coding sessions to finish before Step 2. Each time one finished, it woke, saw
others still running, and armed again. Ten waits in fifty minutes, each ended by
a `CodingAgentIdled` from a different thread, and the eleventh was refused.

The user then tapped **Keep waiting** on the thread's question card. That is a
`UserQuestionAnswered`, not a human `MessageReceived`, so the count stood and
the next wait was refused again.

## Decision

A wait counts toward the cap unless its delivered event is positively
attributed to another thread. The limit rises from 10 to 20 an hour. A human
answer to a question card resets the count, as a human message does.

Attribution reads the matched `events` row. A `ChildThreadCompleted` belongs to
the child it names. Another thread event belongs to its own thread. Anything
else belongs to `actor.source_thread_id` when present, and is unattributed
otherwise.

## Rationale

Neither order nor speed tells a loop from progress. Both are serial, and waiting
on N sessions re-arms as fast as a slow loop. What differs is who woke the
thread. Progress is another thread finishing its own work. A loop is a thread
woken by itself, or waiting for something that never comes (a timeout).

Unattributed events count because the `emit_event` tool writes domain events
with no actor. Reading "unknown" as "someone else" would let a thread that waits
on its own domain event loop unchecked.

Twenty rather than ten, because what still counts includes honest work: a
thread's own background tasks are attributed to itself. A hot loop re-arms in
seconds, so it reaches twenty within a minute or two anyway.

## Consequences

- A thread can wait on other threads' work one at a time for as long as it
  likes, at any pace.
- A thread woken by its own events, by unattributed events, or by timeouts is
  still refused after 20 in an hour.
- **Two threads waking each other are no longer stopped.** Each is woken by the
  other, so neither counts. Only `MAX_LIVE_WAITS_PER_THREAD` remains, and it
  bounds outstanding waits, not a loop. The user accepted this gap.
- The counter stays derived from events: no new state, event or migration.

## Alternatives considered

- **Remove the cap entirely.** Offered to the user and declined: a thread that
  wakes itself would run until someone stopped it.
- **Keep the rate, raise the number.** Tonight's thread would pass at twenty,
  but waiting on thirty sessions would not. It moves the wall without changing
  what it measures.
- **Track causal depth across wakes**, as `max_event_trigger_depth` does for
  triggers. An orchestrator that spawns a session and waits for it adds one
  level per step. A loop does the same, so depth cannot tell them apart.
- **Catch ping-pong by checking whether the waking thread was itself woken by a
  wait.** A coding session woken by its own background task, then finishing, is
  progress, and would count. Rejected as not worth the complexity for a rare
  shape.
