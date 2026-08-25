# 0102: User turns are never summarised, and the conversation summary is cached

- **Status**: Accepted
- **Date**: 2026-08-22

## Context

A chat turn rebuilds `[CONVERSATION HISTORY]` from scratch, in
`engine/chat/process/history.rs`. Past the last 15 messages, everything goes to
an auxiliary model and comes back as one paragraph. That paragraph is not
cached. It is re-rolled on every turn.

One long thread was measured across 24 turns. Only 3 of them produced a
summariser call. An auxiliary `ContextCaptured` row is written only after a
successful response, so a missing row means the call errored or timed out.

On the other 21 turns the prompt carried this:

```
[Earlier (resolved ... do NOT re-attempt fixes described here): (48 earlier messages not shown)]
```

The wording is the harm. The fallback kept the "resolved" framing while
carrying no content at all. So the prompt asserted that four fifths of the
thread was settled, and said nothing about what was in it. That is worse than
sending nothing, because it actively discourages the model from asking.

Success was not sufficient either. One completed call sent 28,128 input tokens
and returned 24 output tokens. Roughly 100 characters stood in for 73 messages.

The whole workspace shows the same shape. Over 30 days, 8 auxiliary memory calls
exceeded 19,000 chars and 939 sat under 15,600. Long threads rarely get a
summary at all.

Sizing matters for what follows. Over the same 30 days, 4,051 user messages had
a median of 63 chars and a mean of 446. On the measured thread, all 55 user
messages together came to 5,337 chars.

## Decision

Three changes to the default context path, plus one scope boundary.

1. **User turns are never summarised.** Only assistant turns are compressed. A
   user turn in the older region renders verbatim, in chronological position,
   bounded by `HISTORY_OLDER_USER_BUDGET` with a count line for any remainder.
2. **The summary is cached as a `ConversationSummarized` thread event.** It
   records the paragraph, the event it covers through, and the model. A later
   turn reuses it and re-summarises only when the uncovered assistant turns
   exceed `HISTORY_SUMMARY_REFRESH_AFTER`.
3. **A missing summary says so, and names the way back.** The "resolved" framing
   survives only where a real summary is present. The no-summary form claims
   nothing and points at `query_events` for the thread.
4. **This binds on the default path only.** ADR 0085's experimental context mode
   keeps its own semantics, where the history is a body that leaves at the round
   boundary.

## Rationale

**This is the floor ADR 0085 deferred, cut to its narrowest useful width.** That
ADR listed "a guaranteed verbatim tail as a floor" under alternatives, deferred
rather than rejected. It then left "the floor" open, for the experiment to
settle. What is decided here is not a tail of N raw messages. It is one role,
kept whole, for the length of the thread.

**0085 already named this as the category that cannot be recovered.** Under what
the notes carry, it lists constraints the user stated in conversation. Each is
said once, it observes, in a message that is then gone. A "do not touch the
frontend" is unrecoverable by any tool call, because nothing else records it. An
assistant turn is different: the work it describes left artifacts, events and
files behind.

**The measurement is what makes the narrow floor affordable.** A median user
message is 63 chars. A whole long thread's user side is about 5 KB. Guaranteeing
the small half costs almost nothing, and it is the half that carries intent.

**Caching converts a coin flip into a ratchet.** At the observed rate the
summariser lands on a minority of turns, and today each miss loses the
paragraph entirely. Cached, the first success holds, and a later failure degrades
to a slightly stale summary rather than to nothing. The fix therefore works
without touching why the calls fail.

**The cache is an event because the alternatives are worse here.** A table is
new schema for derived content. An in-memory cache dies on the restart that
every Apply causes, which is exactly when a user is iterating. `ImageDescribed`
already set the pattern: auxiliary-model output, joined back by an event id,
classified as metadata. `load_chat_history` fetches every thread event anyway,
so the read is free.

**The wording is load-bearing, not cosmetic.** A prompt that asserts resolution
it cannot support is a lie the model has no way to detect. Splitting the
constant is the smallest change that makes the claim conditional on the evidence.

## Consequences

- **The history block grows on long threads.** Older user turns now occupy real
  bytes where they previously occupied none. The measured thread gains roughly
  5 KB, against a 27 KB block. The budget bounds the tail case.
- **The summariser runs less often.** It fires on a refresh rather than every
  turn, which removes a 30k to 100k token auxiliary call from most turn setups.
  That shortens the gap between `MessageReceived` and the first agentic step.
- **A stale summary becomes possible, and is preferred to none.** Between
  refreshes the paragraph lags the assistant's newest aged-out work. Those turns
  render at tier 2 in the meantime, so nothing is silently dropped.
- **Why the calls fail is still unfixed.** The 30 second `AUX_LLM_TIMEOUT`
  against a retrying client, and the payload size, are untouched. Caching makes
  the failure survivable rather than absent.
- **`repeat_recoveries` and the eval are unaffected**, because the lean arm is
  out of scope.
- **A new thread event type joins the wire.** It is metadata, moves no section
  or status, and raises no UI surface.
- **Only a follow-up turn may write the cache.** A new thread's older region is
  a global window over other threads. A row written from it would file their
  content as this thread's own, and its boundary would name an event this
  thread's history never contains. The paragraph still rides that turn; it is
  simply not persisted.

## Alternatives considered

**Raise `HISTORY_RECENT_MESSAGES` above 15.** Rejected as the primary fix. It
buys a bigger verbatim window for both roles and pays for it on every round,
without changing what happens past the new boundary. The cliff moves; it does
not go away.

**Fix the summariser's reliability instead.** Deferred, and offered explicitly
before this scope was chosen. Raising the timeout, budgeting the retries and
chunking the payload would raise the success rate. It would not stop a success
being thrown away and re-rolled. A re-roll is what makes the same thread answer
differently on consecutive turns.

**Summarise user turns too, but with a stronger prompt.** Rejected. It keeps a
model judgment in the path for the one category that has no recovery route. The
observed 24-token output shows what that judgment is worth under load.

**Cache in memory, keyed by thread and boundary.** Rejected. Statelessness
permits a cache, but this one would be cleared by the engine restart that every
Apply performs. That is the moment a user is most likely to be mid-thread.

**Incremental summarisation, folding the delta into the previous paragraph.**
Rejected. It compounds loss across refreshes, which is the failure this decision
exists to stop. A refresh re-reads the assistant turns instead.

**Extend the floor into context mode.** Rejected for now, and the reason is
0085's own. It left the floor to the experiment, and ADR 0087 is that
experiment. A floor written into the lean arm changes what the eval measures,
and buys nothing today, because the flag is off in every workspace.
