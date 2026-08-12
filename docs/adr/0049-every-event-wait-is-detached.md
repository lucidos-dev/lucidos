# 0049: Every event wait is detached: a subscription never holds a turn

- **Status**: Accepted
- **Date**: 2026-08-06

## Context

An *event wait* shipped with two shapes (S5b of
`docs/plans/2026-08-05-a-thread-parks-on-an-event-wait.md`):

- **attached**, the default and the reason the design was thought worth
  building: `await_event` ended the turn with its `tool_use` deliberately
  unpaired, no terminator was written, the thread took a `waiting_for_event`
  status, and the delivered event arrived as that call's `tool_result` so the
  model resumed mid-thought inside the same exchange.
- **detached**, the fallback once anything closed the pair: the thread was an
  ordinary idle thread holding a live subscription, and the delivery arrived as
  a `UserPromptInjected` that started a new turn.

On 2026-08-06 the nightly orchestration thread was force-aborted, and both
halves of that failure were properties of the attached shape alone.

A child E2E thread finished, and **two wakes fired for the same
`ChildThreadCompleted`**: the ADR 0011 child-to-parent fan-in, and the
event-wait dispatcher resolving the parent's own live subscription. The fan-in
wake won the race and started a turn. The event wake was barred from the chat
injection fast path, because an attached wake's prompt is the empty string (the
payload lives in a `ToolResult` the running turn never rebuilt) and an empty
user block is a provider 400. So it took the slow path, blocked in
`register_thread_queued` for exactly 60 s, and force-evicted the running turn
with `ResponseAborted { safety_net }`.

Its restarted turn then anchored on the `await_event` `ToolResult`, which is not
an `EXCHANGE_START_TYPES` member, so its whole output folded into the
abort-boundary exchange as steps, and `ChatExchange` drew none of them. That
turn had applied a change to `main`, spawned a coding-agent sub-thread and
written a full summary, and the UI showed "Response interrupted" with a Continue
button and nothing else.

The attached shape also kept coding agents out of the feature entirely (S11):
the engine does not own a Claude Code or Codex session's message array, so it
cannot leave a dangling `tool_use` in one.

## Decision

Delete the attached shape. `await_event` registers a subscription and returns
immediately, like any other tool; the turn carries on and ends with an ordinary
terminator; the thread is plain `idle` while it watches; and every delivery and
expiry wakes it as a `UserPromptInjected`, injected into a live turn or starting
a new one, exactly as a child completion or a user follow-up does.

`ThreadStatus::WaitingForEvent` is deleted with it. A subscription surfaces
through the per-thread subscription indicator, which S10b had already made the
canonical surface for a wait that did not hold the turn.

## Rationale

**The cost was structural, not incidental.** An unpaired `tool_use` in a message
array is a provider 400 the moment anything else runs on the thread. Everything
built around the attached wait existed to pay for that one fact:
detach-on-interruption with a filler result, an attachment probe re-derived at
every resolution site, a `was_attached` field on all three resolutions, two
wake-anchor shapes the frontend had to tell apart, a blocking status with its
own icon and two SQL mirrors, a restart preserve guard, and the injection
fast-path bar. Removing the shape removes all of it.

**What it bought is smaller than it looks.** "Resumes mid-thought" is about the
shape of the message array, not about whether the event reaches a running turn.
When the thread is already running, a detached wake takes the same injection
channel a user follow-up takes and always could; it was the attached wake that
could not. When the thread is idle, a turn starts either way, and the only
difference is whether the model reads its pre-wait reasoning as a live
continuation or as history, plus one exchange boundary in the transcript. For a
wait that can run up to 24 hours, that boundary is arguably more honest than its
absence.

**The eviction was not a tuning problem.** The 60 s ceiling in
`register_thread_queued` is a backstop against a genuinely stuck turn, and it is
correct. What was wrong is that a wake with nothing to say was pushed into a
queue whose timeout is destructive, when the only reason it could not inject was
a shape chosen upstream.

**It opens the feature to coding agents.** Once a wake is just a follow-up
message, the coding-agent lane already knows how to deliver one (`msg_tx` into a
live session, or a fresh `--resume` when idle, both exercised by the
child-completion fan-in). S11's exclusion was entirely about the dangling tool
call, so it dies with it. Coding agents reach the same registration through
`lucidos await-event` over `POST /api/v1/threads/<id>/event-waits`, deliberately
the same code as the LLM tool so caps and refusals cannot drift between agents.

## Consequences

**Kept.** The subscription itself, unchanged: the `{event_type, condition}`
shape shared with triggers, the one-shot `LiveWaits::take` gate, the watermark
plus catch-up scan that closes the restart gap, the 24 h ceiling, the live-waits
per-thread cap, the duplicate-subscription refusal, the ten-in-a-row loop cap,
and expiry-wakes-rather-than-drops.

**Given up.** The seamless mid-thought resume. A delivery is now always a new
exchange, and the model reads the turn it was in before the wait as history.

**New.** A thread holding a subscription is `idle`: not blocking, not
attention-needing, and archivable (archiving still cancels every live wait, so
that is not a way to strand one). Coding-agent threads can subscribe.

**Migration.** Live rows can carry `status = 'waiting_for_event'` and an
unpaired `await_event` call. A migration rewrites the status to `idle` and
recomputes the blocking counters with the current predicate; a boot sweep
(`settle_legacy_attached_event_waits`) closes the unpaired calls, and
`ThreadStatus::parse` maps the legacy string to `Idle` explicitly so an older
engine sharing the database cannot write a value that reads as unknown.

**Two frontend defects fixed alongside**, because both are latent for any path
that folds a turn into an abort boundary rather than only for this one: such a
boundary now renders the steps it acquired, and the Continue button is withheld
once a boundary has produced a terminal.

**The duplicate wake is still deduped.** All-detached makes the collision
harmless rather than absent, so the fan-in stands its kick down when a persisted
`EventWaitDelivered` names the completion it just emitted. The gate is the
persisted row rather than a cache probe, because the dispatcher resolves off the
same post-commit broadcast the fan-in runs inside.

## Alternatives considered

**Keep both shapes and fix the two defects.** Dedupe the wake, teach the
renderer to draw a turn that lands under an abort boundary, disarm the stale
Continue button. Cheapest diff, and it leaves the whole apparatus in place: the
attachment probe, the filler result, the two anchors, the status with its SQL
mirrors, the preserve guard, the fast-path bar. It also keeps coding agents
locked out, since the obstacle there is the shape itself. Rejected: it treats
the two symptoms and keeps the thing that generated them.

**Keep `waiting_for_event` as a non-blocking, informational status.** The thread
would be idle in every behavioural sense but keep its own drawer icon and label.
Smaller diff, no contract regeneration, and it preserves a good piece of copy
("Asleep until something it subscribed to happens"). Rejected because a status
that never affects behaviour is a second, weaker surface for something the
subscription indicator already shows per thread, and keeping it would leave the
enum carrying a value no rule reads.

**Raise or remove the `register_thread_queued` timeout.** Would have prevented
this particular eviction and nothing else: the wake still could not inject, the
turn still folded into the wrong exchange, and a genuinely stuck turn would then
block a queued follow-up indefinitely. Rejected as papering over the upstream
shape, which is what `.claude/rules` calls fixing the symptom.

**Make the attached wake carry its payload as a prompt too, so it can inject.**
That is detached with extra steps: the tool result would be written and the same
content sent as a user block, so the model reads it twice and the "no exchange
boundary" property, the only thing attachment buys, is gone anyway.
