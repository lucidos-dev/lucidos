# 0089: A Claude SSE stream that ends before message_delta with nothing salvaged is a transport truncation, so it retries instead of failing the turn as an empty completion

- **Status**: Accepted
- **Date**: 2026-08-18

## Context

A chat turn on Vertex died mid-flight with `Model returned no response
(stop_reason: unknown, output_tokens: ?, thinking_chars: 0)`. The engine had
sent round 16 of the turn, and Vertex answered four seconds later with the
`message_start` frame alone: input cost billed, then EOF. No content block, no
`message_delta`, no `error` frame, and nothing dropped by the parser.

`parse_claude_stream` returned `Ok` with every field empty, because a truncated
stream parses fine. It just holds nothing. The retry loops in
`llm/vertex/claude.rs` and `llm/anthropic/chat.rs` only fire on `Err`, so
nothing retried. The empty response reached `classify_empty_completion`, which
correctly refused to vouch for an unrecognised stop and emitted
`ResponseFailed` (ADR 0009). The user lost the turn to a provider blip that a
single retry would have survived.

## Decision

`parse_claude_stream` returns `Err` when the stream ends with **no stop reason
and no output**, naming it a truncation. `is_transient_error` classifies that
message as transient, so both callers retry it on their existing path and the
scheduler suppresses duplicate trigger notifications.

Output means text or a tool call. Thinking does not count: it never reaches the
frontend, and the engine keeps only its length.

## Rationale

- A whole Anthropic stream always ends `message_delta` then `message_stop`, and
  `message_delta` is what carries the stop reason. Its absence is not
  ambiguous. The turn did not end, the connection did.
- The distinction is transport versus semantics. An empty completion is a
  statement the model made about its turn. This is the absence of any statement
  at all, which is the transport layer's problem to report.
- Where the error is raised decides whether it retries. Both callers already
  retry a transient stream error with backoff, so raising it at the parse
  boundary needs no new machinery.
- The classifier stays honest for the case it owns. ADR 0009 keeps failing an
  unknown stop safe to `ResponseFailed`; it now sees that case only when the
  provider really did report an unrecognised stop.

## Consequences

- A dropped stream costs a retry (up to `MAX_RETRIES`, 2s backoff doubling)
  rather than the turn. The retried request bills its input tokens again, which
  is what every existing stream-error retry already does.
- A provider that truncates persistently still surfaces, as `Claude stream
  truncated: … (after N attempts)`. That is a sharper diagnosis than `Model
  returned no response`, which named a symptom.
- The failure shape is rare enough to have appeared once across the 200 most
  recent `ResponseFailed` events. This is a resilience fix, not a hot path.
- **Narrowed by ADR 0091.** The `carries_output` guard also excluded a stream
  cut mid tool-arguments, and the reason below does not hold there: nothing of
  a tool call reaches the frontend. That case is now classified by stop reason
  rather than by whether a block was produced.
- **Extended to the OpenAI paths by ADR 0099.** This decision was implemented
  for `parse_claude_stream` only, and an OpenAI-compatible route later ended a
  thread in silence the same way. The signal there is a missing usage block
  rather than a missing stop reason, because that stream reported
  `finish_reason: stop`.

## Alternatives considered

- **Retry every stream that lacks a stop reason, including one that already
  carried text.** Rejected: the token callback has already streamed that text
  to the frontend, so a retry renders the answer twice. Partial content stays a
  success and the caller decides what to do with it.
- **Treat the unknown stop as retryable inside `classify_empty_completion`.**
  Rejected: that classifier runs after the provider returned, in the agentic
  loop, which has no way to re-issue the request. Retry belongs to the layer
  that owns the connection.
- **Give the user a Continue button and leave the engine alone.** Rejected: the
  thread already has one, and it makes the user pay attention to an
  infrastructure hiccup the engine can absorb.
- **Count `message_stop` instead.** Rejected: the parser ignores that frame
  today, and the stop reason is the field the rest of the engine reads. Gating
  on the value we actually consume keeps one source of truth.
