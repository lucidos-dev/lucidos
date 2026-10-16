# 0167: A call makes the thread real, and moves nothing a turn owns

- **Status**: Accepted
- **Date**: 2026-08-30

## Context

Every voice event is `EventClass::Metadata`, and its projection arm moved no
column at all. Only `MessageReceived` flips `thread_summaries.state` from
`composing` to `active`, and only a delegation writes one.

So a call the talker answered by itself left the thread a draft. The user
placed one from the compose view, talked for half a minute, hung up, and the
whole transcript stayed inside `CreateThreadView`. The drawer showed "Empty
draft". Nothing said a conversation had happened.

ADR 0148 already promised the opposite: "Voice inherits thread titling, thread
listing, search and triggers for free." It inherited none of them on the one
path where the doer never ran.

## Decision

The first word spoken on a call promotes a `composing` row to `active`, clears
the compose fields, bumps the *compose epoch* and broadcasts the clear. Either
spoken row does it, because either can be the first.

Both also set `has_response`, and the caller's first words fill
`first_message`. A `VoiceSessionStarted` carries recency and nothing else.

Nothing here touches `status`, `source` or `message_count`.

## Rationale

**The first spoken word is the right moment, and `has_response` is why.**
`get_recent_threads` filters on that column, so promoting a draft without
setting it produces a thread that is neither a draft nor a listed thread. That
is strictly worse than the draft it replaced. `has_response` is a listing
decision rather than a literal response, which is why `ResponseCanceled` and
`ResponseFailed` set it too.

**So connecting cannot be the moment.** A call that drops before a word is said
has nothing to list. Promoting there would leave an unreachable row, where
leaving the draft alone leaves something the reader can see and discard.

**The columns split by who owns them.** ADR 0149 gives the doer's turn the
thread's status, and ADR 0148 says a live microphone is not a turn. Both still
hold: the promotion is about whether the thread EXISTS, which is a different
question from whether anything is running in it. `message_count` counts turns
the agent ran, and a call runs none.

**Recency is not status.** Without the bump a long call leaves the thread
sorted at the instant it was created. On an existing thread a talker-only call
would never re-sort the drawer at all. The user just spent ten minutes there.

**`first_message` is the whole of the titling fix.** The server's
`format_display_title` already falls back to it, so one column stops a
voice-only thread reading "Untitled Thread". No title machinery is added.

## Consequences

- A call always leaves something a user can find, whether or not it delegated.
- The first spoken word consumes the compose slot, exactly as a send does, so a
  device holding that draft is told at once.
- A call that connects and says nothing leaves the draft untouched.
- A thread promoted this way carries `message_count = 0` and no generated
  title until its first delegated turn. The `first_message` fallback covers
  the row; a real LLM title is deferred.
- A migration promotes the drafts the old projection already stranded, and
  backfills their `first_message`.
- `LAST_ACTIVITY_EVENTS` gains the three events, so the frontend's optimistic
  `updatedAt` and the backend column stay in step.

## Alternatives considered

**Promote on `VoiceSessionStarted` instead.** One arm rather than two, and it
puts the pane swap at the start of the call. Written that way first, and
rejected during hardening: `get_recent_threads`'s `has_response` gate makes a
wordless call an unreachable row. Setting `has_response` at connect time would
be a lie. Demoting the row at hangup would be a transition nothing else in the
state machine has.

**Promote in `api::voice::admit` rather than the projection.** Rejected: it
puts thread state outside the event log, so a projection rebuild would undo it.
The arm is deterministic on replay.

**Leave `state` alone and teach the drawer to show a draft holding voice
events.** Rejected: it makes "draft" mean two things, and every consumer of
`state` would need the second one. The thread really is not a draft any more.

**Bump `message_count` for a spoken row.** Rejected: the column counts turns
the agent ran, and it feeds "is this a follow-up" decisions elsewhere. A
talker-only utterance ran nothing.

**Let the promotion also set `status = 'running'` while the call is up.**
Rejected outright: it is the exact claim ADR 0148 refused when it made both
session events `Metadata`. A thread reading `running` with no turn behind it
would light the drawer and mislead every recovery sweep.
