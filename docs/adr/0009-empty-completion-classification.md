# 0009 — An empty completion is an error only when it's genuinely an error, classified per-cause across providers

**Status:** Accepted (partially reverses the "all empty completions are
`ResponseFailed`" stance of `fe553f4b2`). **Narrowed by
[0089](0089-a-truncated-stream-is-a-transport-error.md)**: a Claude stream that
ends with no stop reason and nothing salvaged is a transport truncation. It
retries, so it never reaches this classifier as an `Unknown` stop.
**Narrowed again by [0099](0099-openai-stream-with-no-usage-is-a-truncation.md)**:
an OpenAI stream with no output and no usage block is the same truncation. A
clean stop now reaches the benign branch only when the stream really completed.

**Date:** 2026-06-14

## Context

When the chat agentic loop ends a turn with no text and no tool calls (an
*empty completion*), the engine has to emit some terminal event. The history:

1. The loop used to synthesise the literal string `"Done."`. That contaminated
   memory indexing, parent-callback summaries, and trigger result summaries —
   and once fooled an orchestrator into concluding its pipeline was complete
   after step 1 (the fake `"Done."` read as a real child result).
2. `5d39e7226` replaced the fallback with explicit events: clean stop →
   `ResponseEmpty`, truncation → `ResponseFailed`.
3. `fe553f4b2` then **collapsed** `ResponseEmpty` into `ResponseFailed`, on the
   grounds that "the model returning no content is a failure from the user's
   perspective" and "the UI styled both as errors anyway."

The trigger for revisiting: a Sonos chat turn where Gemini ran the tool calls
successfully (the music played), then returned `finishReason: STOP` with no
text. The engine emitted `ResponseFailed` → a red error dot on a turn that
*succeeded*. Two problems surfaced:

- The blanket "empty = failure" rule is wrong when the model finished cleanly
  and simply had nothing to add.
- The failure-detection was written in Anthropic's stop-reason vocabulary
  (`max_tokens`, `refusal`). Provider finish reasons are raw and unnormalized —
  Gemini uses `STOP` / `MAX_TOKENS` / `SAFETY`, OpenAI uses
  `completed` / `length` / `content_filter`. A classifier keyed on Anthropic's
  strings would mis-file every other provider's truncation and safety stop —
  and the report was Gemini. `thinking_chars` / `unknown_sse_dropped` are also
  Anthropic-only (Gemini/OpenAI hardcode them), so the parser-miss heuristic is
  dead for non-Anthropic providers.

## Decision

Classify an empty completion by **why** it was empty, uniformly across
providers and thread types (chat, trigger, orchestrator — the launcher does not
matter):

- **Clean** model-decided stop with nothing dropped → **benign**: emit an empty
  `ResponseGenerated`. The thread completes Idle (no red dot); the frontend
  renders a neutral "the model returned an empty response" note.
- **Truncated** / **Blocked** / **dropped output** / **Unknown** stop reason →
  **`ResponseFailed`** (red), as before.

Stop reasons are normalized once (`normalize_finish_reason` →
`FinishClass::{Clean, Truncated, Blocked, Unknown}`, case-insensitive) so the
decision is provider-agnostic. `Unknown` (null, `stop_sequence`, an unmapped
future value) fails **safe** to `ResponseFailed`. Implementation lives in
`crates/lucidos-engine/src/engine/agentic_loop/helpers.rs`
(`classify_empty_completion`) and the empty branch of `agentic_loop/run.rs`.

## Rationale

- "It's an error when it's actually an error" is the honest rule. A clean stop
  with no text is the model choosing silence — visible to the user as a note,
  not alarming as a red error. Truncation, a safety block, or lost output are
  real failures and stay red.
- Reusing `ResponseGenerated` (rather than reviving `ResponseEmpty`) keeps the
  event taxonomy unchanged — no new variant, no contract regen, and every
  existing consumer (projection → Idle, `parent_callback` → `Success`,
  reconstruct, memory) already handles it. The distinct *rendering* the
  `fe553f4b2` collapse lacked is supplied by a frontend `{ type: 'empty' }`
  response-event, not a new domain event.
- The original orchestrator bug does **not** regress: an empty
  `ResponseGenerated` yields an empty summary, never a fabricated `"Done."`.

## Consequences

- Empty `ResponseGenerated` now occurs in normal operation. `reconstruct.rs`
  surfaces it as `You (assistant): (no text response)` so an orchestrator still
  sees the gap without it being labeled a failure.
- A benign-empty **child** thread now reports `Success` (empty summary) to its
  parent instead of `Failure`. This is intentional under the per-cause rule.
- A trigger that returns a clean-but-empty turn no longer auto-fails. Genuine
  failures (truncation/safety/dropped/unknown) still do, so the scheduler's
  error notification path is preserved for real problems.
- Normalizing stop reasons also fixes the previously Anthropic-only diagnostic
  hints for Gemini/OpenAI failures.

## Alternatives considered

- **Keep "all empty = `ResponseFailed`" (status quo).** Rejected: red-flags
  successful turns; the report is a direct counterexample.
- **Revive the `ResponseEmpty` event variant.** Rejected: ~50 engine match
  sites + contract regen + a re-litigation of `fe553f4b2`, for rendering that a
  frontend-only response-event achieves.
- **Scope the benign treatment to interactive chat; keep triggers strict.**
  Rejected in dialogue: the determinant should be the *cause* of the emptiness,
  not what launched the thread. `did_work` (tool calls ran) and the normalized
  stop class already capture the real signal; a chat-vs-trigger gate added
  branching for no correctness gain.
- **Reuse `ResponseGenerated` with a synthesized note in `text`.** Rejected:
  re-introduces the `"Done."`-style contamination into memory / parent
  summaries. The note must be presentation-only.
