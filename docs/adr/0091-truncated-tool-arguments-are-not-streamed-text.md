# 0091: A truncated tool-argument stream is classified by why the stream stopped, not by whether a block was produced

- **Status**: Accepted
- **Date**: 2026-08-18

## Context

A chat turn died with `Failed to parse tool arguments: EOF while parsing an
object at line 1 column 149`. The model was writing a large document through
`write_file`, and the accumulated `input_json_delta` string was a well-formed
prefix: `path` and `message` complete, `content` never started.

The exported thread measures the round. It ran 386 seconds against a median of
5.6 for that thread, the 300 second per-chunk timeout never fired, and nothing
retried. Six and a half minutes of continuous generation, ending forty tokens
into a tool call, is the model exhausting `max_tokens` rather than a connection
dropping.

Two layers failed independently:

- `thinking_config` hardcoded `max_tokens: 32768` for every adaptive-thinking
  model at every effort. The cap bounds thinking and response text together, so
  a deep turn spends it before writing anything. Opus 5 publishes a 128k output
  ceiling, so the engine was imposing a budget nobody asked for.
- `parse_claude_stream` turned any `serde_json` failure on the arguments into
  one hard error, and `is_retryable_error` matched none of that text. ADR 0089's
  truncation guard could not reach it either: that guard skips any stream where
  `carries_output` is true, and a `ToolUse` block always is.

## Decision

Two rules.

**Each adaptive-thinking model carries its own published `max_tokens` ceiling**,
paired with the model fragment in one `ADAPTIVE_THINKING_MODELS` table.
`requires_adaptive_thinking` derives from that table, so the ceiling and the
adaptive gate cannot drift.

**A tool-argument parse failure is classified by `stop_reason`**, not by whether
a block was produced:

| `stop_reason` | Meaning | Behavior |
|---|---|---|
| absent, nothing rendered | the connection ended mid-arguments | retryable truncation |
| absent, text already rendered | same cut, but the turn had spoken | reports, does not retry |
| `max_tokens` | the model ran out of budget | names the limit, not retryable |
| anything else | the model emitted malformed JSON | today's message, unchanged |

"Rendered" means the token callback pushed text to the frontend. A caller that
passes no callback has rendered nothing, so its text only exists in the response
a retry discards, and that case stays retryable.

## Rationale

- **ADR 0089's reason for keeping partial text does not extend to a partial
  tool call.** It kept partial text because the token callback already streamed
  it, so a retry renders the answer twice. Nothing of a tool call reaches the
  frontend, and the tool never ran, so a retry duplicates nothing. The guard
  excluded this case by accident, through `carries_output`, rather than on its
  own argument.
- **That reasoning is about the tool call, not the turn.** A turn can stream a
  sentence of preamble and then cut inside the tool call it announced. The
  first draft retried there and would have rendered the preamble twice, which
  is the outcome ADR 0089 rejected by name. So the retryable arm is gated on
  nothing having been rendered, not on the tool call being invisible.
- **`stop_reason` is the only thing that says whether a retry can help.** A
  dropped connection is transport and survives one attempt. A budget cut is
  deterministic, so an identical retry is cut identically and burns the budget
  again.
- **Too high a `max_tokens` is worse than too low.** The API rejects the
  request, so every turn on that model fails rather than one long one. Pairing
  each fragment with its ceiling means a new arm cannot be added without naming
  one, and an unlisted id stays off the adaptive path.
- **The ceiling is verified, not read off a docs table.** Vertex accepts
  128000 and rejects 128001 with `max_tokens: N > 128000, which is the maximum
  allowed number of output tokens`. Opus 4.7, Opus 4.8, Opus 5 and Sonnet 5
  each report that number. So "128k" is decimal, and we sit exactly at the
  ceiling rather than under a guess.
- **A non-retryable arm verifies its own classification.** `is_retryable_error`
  reads standalone alphanumeric tokens, and every non-retryable arm
  interpolates model-supplied text. A tool name may legally be `502`, raw
  arguments may contain one, and a serde message ends in a column number. Each
  arm therefore checks its own message and falls back to fixed wording when the
  detailed one would read as transient.
- **The engine layering already forbids the obvious shortcut.** `llm` must not
  reach up into `engine::*`, so the wire parser matches the literal
  `"max_tokens"` rather than calling `normalize_finish_reason`. This file is
  Anthropic-specific, so Anthropic's vocabulary belongs in it.

## Consequences

- A dropped connection mid tool-call costs a retry rather than the turn. Both
  the Vertex and direct Anthropic paths get it, since both consume the shared
  parser.
- A budget cut still fails the turn, which is honest. It now says what happened
  and what to do, rather than showing a serde error about a JSON blob the user
  never wrote.
- An adaptive turn can now generate up to 128k output tokens. That is a ceiling
  rather than a spend, so billing follows what the model actually produces. A
  pathological turn can still run longer before anything stops it, though the
  per-chunk timeout continues to cover a stall.
- Adding an adaptive model means adding a row with its ceiling. Forgetting the
  row leaves the model on the `budget_tokens` path, which is a degradation
  rather than an outage.

## Alternatives considered

- **Raise `max_tokens` and stop there.** Rejected: it makes the failure rarer
  without making it survivable, and the same shape returns on any dropped
  connection, which no cap prevents.
- **One flat constant for every adaptive model.** Rejected: it is the same
  arbitrary number that caused this, one size up. A future adaptive model with
  a smaller ceiling would also inherit it and fail every request.
- **Read the ceiling from the model registry.** Rejected for now: `ModelRouting`
  carries `context_window` only, and `thinking_config` never sees the registry.
  Worth revisiting if per-model output limits become user-editable, which would
  need a column, a migration and a settings surface.
- **Retry the `max_tokens` cut as well.** Rejected: nothing about the retried
  request differs, so it fails the same way and bills the full budget twice.
- **Widen `carries_output` so an incomplete `ToolUse` does not count.**
  Rejected: it would cover the dropped-connection case but not the budget one,
  and it decides retryability from block shape rather than from why the stream
  stopped.
- **Drop the incomplete tool call and let `classify_empty_completion` report
  the truncation.** Rejected: it only reaches that classifier when the turn is
  otherwise empty. A turn with text plus a cut tool call would return `Ok` and
  the truncation would vanish.
