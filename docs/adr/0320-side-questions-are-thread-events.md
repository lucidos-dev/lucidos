# 0320: A /btw side question and its answer are recorded as four quiet thread events that no agent, trigger, event wait or query_events reader ever sees; supersedes 0318

- **Status**: Accepted
- **Date**: 2026-09-28

## Context

ADR 0318 kept side questions in client memory only, so no context builder could
read them. The user found the cost on their phone: the iOS home-screen app is
evicted from memory often, and every eviction, reload or dismiss lost the card
for good. They asked for side questions to be kept as events, and to be able to
get a dismissed one back.

A survey of the engine's thread-event readers found two kinds. Every reader that
builds agent or LLM context picks event types from a fixed list: session history,
the stale-resume recap, chat context, the conversation summary, working
understanding, titles, memory, search. A new variant is excluded from them by
construction. A handful read every type: the `query_events` store function
behind the agent's tool and the CLI, trigger matching, event waits, the external
watchdog and two "latest event" checks.

## Decision

A side question is recorded as four persisted `ThreadEvent`s:
`SideQuestionAsked`, `SideQuestionAnswered`, `SideQuestionFailed` and
`SideQuestionDismissed`, keyed by a client-named `side_question_id`. They move no
thread state. `ThreadEvent::is_side_question_event` and `SIDE_QUESTION_EVENT_TYPES`
mark them, and every reader that takes all types excludes them.

## Rationale

1. **The allowlists already do the hard part.** 0318's rationale 2 feared a
   filter in every context builder. The survey showed those builders never read
   an unknown type, so only the few generic readers need a filter.
2. **One predicate, one list.** Each generic reader names the same constant, and
   a test per reader pins the exclusion. A new generic reader copies the pattern.
3. **Events are the source of truth.** A card that survives reload and shows on
   every device is what an event gives for free. Local storage would stay
   per-device and lose a dismissed card for good.
4. **Dismissing keeps the ask.** The card collapses to a row the user can reopen,
   which is what "get it back" asked for.

## Consequences

- Cards survive reload, iOS eviction and device changes, at the seq they were
  asked at.
- Startup records `SideQuestionFailed` for every ask a restart left unanswered,
  so no card waits forever.
- A subscription on a side-question event is refused with a message, and the
  events are absent from `query_events`, counts and the distinct-type list.
- Token Cost still cannot count side questions: Claude Code reports no usage.
- A new reader of every event type must exclude these, or it leaks them. Each
  reader that does is pinned by a test fed `every_side_question_event()`.
- The ask runs detached from its request, so a reload mid-ask still records the
  answer. A cold process can therefore outlive its asker, up to the shared
  answer deadline.

## Alternatives considered

- **Keep 0318: memory only.** Lost on every reload and eviction, which is what
  the user reported.
- **Client storage (localStorage).** Survives a reload on one device only, and a
  dismissed card is still gone for good.
- **A separate table outside thread events.** Nothing could leak, but it adds a
  store, an API and its own sync, beside the event log that is already the
  source of truth. The user chose events.
