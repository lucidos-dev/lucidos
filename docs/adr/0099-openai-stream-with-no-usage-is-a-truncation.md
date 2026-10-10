# 0099: An OpenAI stream that ends with no output and no usage block is a transport truncation, not an empty completion

- **Status**: Accepted
- **Date**: 2026-08-22

## Context

A chat turn on an OpenRouter-style route ended with no message and no red dot.
The thread read as finished, so the user had to ask why it stopped.

The stream returned no text, no tool call, and no usage block. It did report
`finish_reason: stop`. `classify_empty_completion` reads output tokens through
`response.output_tokens.unwrap_or(0)`, which flattens "the provider sent no
usage at all" onto "the provider said zero output tokens". A clean stop with
nothing dropped is benign under ADR 0009, so the engine emitted an empty
`ResponseGenerated` and the thread went Idle.

The same route had thrown `OpenAI streaming error [unknown]: ERROR` twice in
that thread sixteen minutes earlier. An upstream proxy whose error frame
carries the literal string `ERROR` is not healthy. Its silence should not read
as the model choosing to say nothing.

ADR 0089 already settled this class for Claude. It was never ported, and its
signal does not transfer: 0089 keys on a missing stop reason, and this stream
had one.

## Decision

`build_llm_response` (`llm/openai/mod.rs`) returns `Err` when a stream carried
no content, no tool call, and no usage block. The message contains `stream
truncated`, so `is_transient_error` classifies it retryable and both callers
absorb it on their existing retry path.

Output means text or a tool call, matching ADR 0089. A usage-less stream that
produced either one stays a success.

No usage block means neither token count arrived. Either one on its own proves
the terminal frame reached us, so a server reporting half the pair is odd
rather than truncated.

## Rationale

- Both OpenAI paths are guaranteed a usage block by construction. Chat
  Completions sends `stream_options.include_usage` unconditionally, and the
  Responses path reads usage off the `response.completed` terminal event. So
  no usage means the terminal frame never arrived.
- A `finish_reason` on an intermediate chunk is not proof the stream finished.
  The terminal frame is, and its absence outranks what an earlier chunk
  claimed.
- Raising it at the parse boundary needs no new machinery, which is the same
  reason ADR 0089 chose that layer. The retry loops already exist.
- The classifier stays honest for the case it owns. ADR 0009 keeps a clean stop
  benign, and now sees that case only when the stream really did complete.
- Silence is the worst failure shape available. A red error with a Continue
  button costs a glance, while a thread that ends quietly costs a
  conversation.

## Consequences

- A dropped stream costs a retry rather than the turn, billing its input tokens
  again. Every existing stream-error retry already does this.
- A route that truncates persistently surfaces as `ResponseFailed` naming the
  truncation, instead of an Idle thread with no assistant message.
- A non-compliant local server that never sends usage will retry and then fail
  any turn where the model also produced nothing. That turn was already
  degenerate, and the resulting message names the cause. Reconsider this if
  such a server becomes a supported target.
- The guard depends on `include_usage` staying unconditional. Making it
  optional would silently disarm this, so that request line and this rule move
  together.

## Alternatives considered

- **Port ADR 0089 literally, keying on a missing stop reason.** Rejected: the
  incident stream sent `finish_reason: stop`, so the port would not have caught
  the case that prompted it.
- **Teach `classify_empty_completion` to tell absent usage from zero usage.**
  Rejected for the reason ADR 0089 gives: that classifier runs in the agentic
  loop, which cannot re-issue the request. It would turn a recoverable blip
  into a red error the user has to clear by hand.
- **Retry any stream with no usage block, including one that carried text.**
  Rejected: the token callback has already rendered that text, so a retry shows
  the answer twice. This is ADR 0089's rejected alternative, unchanged.
- **Treat a usage-less stream as benign but add a warning log.** Rejected: the
  log is exactly what nobody reads. The incident was diagnosed from the event
  store, days later, only because the user asked.
- **Fall back to another route on repeated truncation.** Rejected as a separate
  concern. Route health belongs to the model registry, not to a stream parser,
  and nothing here blocks adding it later.
