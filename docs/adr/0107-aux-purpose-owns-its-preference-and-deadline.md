# 0107: Every auxiliary model call owns one preference pair and a deadline that contains its retries

- **Status**: Accepted
- **Date**: 2026-08-22

## Context

An *auxiliary model call* is one the engine makes for itself: a thread title, an
image description, fact extraction, query classification, a conversation
summary. Each is stamped with a `ContextPurpose` and each reads a model
preference.

Neither was one-to-one. `ContextPurpose::Memory` stamped three jobs, and
`model_memory` named the model for all three. Two costs followed.

Settings could not name the summariser. Its row read "Memory & context", so a
user looking for the thing that writes their conversation summary had nothing to
find.

The wire could not tell the jobs apart either. A 94,903-char summariser call and
a 6,800-char extraction arrived under one `purpose`, so diagnosing the summariser
meant inferring it from payload size.

Separately, its two reliability knobs were compiled in. The effort was a literal
`Some("low")`, and one 30s `AUX_LLM_TIMEOUT` covered every auxiliary call. The
provider retries three times behind 1s, 2s and 4s of backoff, over a client with
a 900s per-request timeout. So the deadline could only ever cut the FIRST
attempt off, and the three retries it was paying for never happened. On one
thread the summariser landed 3 times across 19 eligible turns.

## Decision

Two standing invariants, both unit-tested in `engine::aux_purpose`.

**One `ContextPurpose` per auxiliary model preference.** A purpose names exactly
one `model_x`, and no two purposes name the same one. History summarisation
became `ContextPurpose::ConversationSummary` with `model_conversation_summary`;
`Memory` kept extraction and classification.

**A purpose's deadline holds one full attempt plus the whole backoff.** Each
purpose declares a deadline AND a per-attempt HTTP timeout, and
`attempt_timeout + total_backoff <= deadline` is asserted for every purpose,
along with `attempt_timeout < deadline`. The aux providers take that attempt
timeout through `with_request_timeout`. So the bound is real rather than
aspirational, and every site declaring a deadline now applies it.

**One attempt, not all four.** Bounding all four forces `attempt_timeout` down
to a quarter of the deadline, which is worse than bounding none: an attempt
shorter than the call's real latency turns one success into four guaranteed
failures. The observed failure is a transport error returning in milliseconds,
so the backoff is what needs room, not four full attempts. A server that hangs
four times over consumes the deadline, which is what a deadline is for.

Every purpose also stores its effort beside its model, as a `reasoning_x`
sibling defaulting to whatever its call site used to hardcode.

## Rationale

**The naming half is not cosmetic.** A preference the user cannot find is a
preference they cannot use. A purpose stamping three jobs also makes the
cheapest diagnostic, reading the `purpose` column, useless. Both failures come
from the same many-to-one, so the invariant is one-to-one rather than a better
label.

**The deadline half is arithmetic, not judgment.** A deadline shorter than the
retry budget does not merely shorten the call: it converts a four-attempt policy
into a one-attempt policy, silently. Nothing in the old code said so, because
the deadline lived in `chat/process_helpers.rs` and the retry budget lived in
`llm/mod.rs`. Pairing them in one struct with a test is what makes a future
change to either side fail loudly.

**Per-purpose, because the calls are not alike.** The summariser ships tens of
thousands of tokens and runs on a refresh. Classification ships a sentence and
runs on every turn with the user waiting. One number could serve either well,
never both.

## Consequences

- Adding a `ContextPurpose` variant fails the tests until it declares its
  preference pair and its budget. That is the point.
- Five paired keys exist where there were four model keys, and every default
  reproduces the literal it replaced. Exactly one behaviour changed with them:
  image description passed no effort, which Gemini reads as `high`, so
  captioning was buying the model's deepest thinking. It defaults to `none` now.
- The command guard's judge is an auxiliary call with NO purpose, so it emits no
  `ContextCaptured` and its tokens go unaccounted. The invariant does not cover
  it, and the module says so. Giving it a purpose is the obvious follow-up.
- Historical rows keep `purpose = memory`, including for summariser calls made
  before the split. `core::aux_context_backfill` deliberately grew no
  `ConversationSummary` arm: reconstructing old calls under the newer purpose
  would relabel the past.
- The deadline numbers are provisional. We had token counts and no latencies, so
  each call now logs its elapsed time and the next pass tunes from data.

## Alternatives considered

**Leave the purposes merged and just rename the Settings row.** Cheapest, and it
fixes the half a user sees. Rejected because the wire stays ambiguous, so the
next diagnosis is the same guessing game. The summariser also still could not be
pointed at a different model from the extractor.

**Raise `AUX_LLM_TIMEOUT` and change nothing else.** A 90s constant would let the
summariser finish. Rejected because it would put the per-turn classification a
user waits on behind the same 90s. The retry budget would also still be
unbounded relative to it, and the pairing is what makes the number defensible.

**Lower `MAX_RETRIES` for auxiliary calls instead of bounding the attempt.** It
would fit the budget inside the deadline too. Rejected because `MAX_RETRIES` is
global and shared with the chat path, and a second retry policy is a second
thing to keep in step. A per-provider request timeout is local to the instance
the aux path already builds per model.

**Store the pair as one JSON preference value.** It would make a half-applied
selection unrepresentable. Rejected because `PrefValue` has no object variant,
and `chat_model` / `chat_reasoning_effort` and `TriggerConfig::model` /
`reasoning_effort` already resolve independently. A third storage shape for the
same concept costs more than the invariant it buys.

**Raise the summary effort from `low` instead of fixing the deadline.** The
obvious first guess. Rejected on the measurements: across five successful calls
at `low`, the three largest inputs produced the three longest paragraphs, so
output length does not track the setting. The failures were calls that never
completed, and one thin roll that the cache then held for five turns. The floor
in `summarize_or_none` addresses the second; the deadline addresses the first.
Keeping `low` is what lets either be measured on its own.
