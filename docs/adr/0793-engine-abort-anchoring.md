# 0793: An abort emitted outside the agentic loop anchors on the request event id its running turn recorded

- **Status**: Accepted
- **Date**: 2026-11-22

## Context

Some aborts are emitted from outside the agentic loop: restart teardown,
stuck-turn eviction and the shutdown sweep.

Each abort must carry the `request_event_id` of the turn that is actually
running. The loop stamps the same id on its own terminator.

The idempotency gate in `thread_events::emit_response_canceled` compares the
two ids. If they differ, the loop's cancel lands as a second boundary.

This record restates the design comments of
`crates/lucidos-engine/src/engine/mod.rs`, written before the comment strip
removes them. Nobody checked it line by line against the code.

## Decision

- The running turn records its own `request_event_id` on its `ThreadHandle` as
  soon as its originating event is resolved, through
  `LucidosEngine::set_thread_request_event_id`.
- `in_flight_request_event_id` reads that handle value first. It falls back to
  `agent_session::latest_originating_event_id` only when no live handle exists,
  or the turn has not yet resolved its originating event.
- The write is guarded by the caller's `ThreadGuard::generation`, so a
  registration that has moved on cannot stamp the handle.
- The stuck-turn eviction emits a `ResponseAborted` with actor System before it
  evicts the turn.
- `register_thread_queued` measures its wait budget per holder, not per caller.

## Rationale

The handle value is authoritative because the turn wrote it. It is the same id
the loop stamps on its own terminator, so the gate recognises the abort and
skips the loop's cancel.

`latest_originating_event_id` only guesses. It returns the newest
originating-type event on the thread. That is the wrong turn when the user
queued a follow-up mid-turn, because the queued `MessageReceived` is newer but
never anchors a turn. It is also wrong when the turn started from an event the query does not name. Examples are `ContinuationStarted` for a chat Continue and `ContinuationRequested` for a coding-agent resume.

A mis-stamped abort defeats the gate. The transcript then shows "Paused by
restart" and "Response canceled" stacked. The frontend's
`Aborted`-before-`Canceled` check in `exchangeStatus` only removed the
duplicate when both landed on the same exchange.

The generation guard exists because a force-evicted turn keeps unwinding while
its replacement is already registered under the same `thread_id`. A bare `thread_id` lookup would let the dying turn stamp over the live one. The next abort would then end the turn the user already abandoned.

The eviction pre-emit exists because the run-loop's stop arm would otherwise
default to `ResponseCanceled`. The user would see a misleading "Canceled" after
the 60 s timeout.

The budget is per holder because a caller must not inherit impatience earned
against work that has already finished. Each time a different turn takes the
thread, the wait starts again.

## Consequences

- The handle value never needs clearing. The handle is the turn, and
  `ThreadGuard::drop` removes it, so the value cannot outlive what it
  describes.
- The fallback is still a guess, but a guessed anchor beats none. A `NULL`
  `request_event_id` breaks the Continue window in `chat/rerun.rs` and the
  frontend's exchange grouping.
- The evicted turn's own anchor is read from its handle, which is still
  registered because the caller evicts after the emit.
- The fallback event lists cover turns that never recorded an anchor. A
  coding-agent thread uses `MessageReceived`, `CodingAgentUserMessageSent`,
  `TriggerStarted` and `ChildThreadCompleted`. A chat thread uses the same list
  minus `CodingAgentUserMessageSent`. The shared constants live in
  `agent_session::resume`.
- Coding-agent sessions also get `external_terminal_emitted` set, so the
  run-loop arm skips its duplicate emit.
- `announce_eviction` runs even when the eviction is then refused. The cost is
  at most a duplicate terminator on the wedged exchange, which the emit gate
  drops, and never a missed one.
- The wait body is a free function, so the engine method and its tests exercise
  one implementation. The same holds for the handle write.

## Alternatives considered

- Anchoring every external abort on
  `agent_session::latest_originating_event_id` alone. It names the wrong turn
  when a follow-up is queued or an unlisted event started the turn.
- Letting the run-loop's stop arm emit the terminator for an evicted turn. It
  would default to `ResponseCanceled` and show a misleading "Canceled".
- Measuring the wait budget per caller. A caller would inherit impatience
  earned against work that has already finished.
