# 0255: An engine re-entry, such as a child's completion, never supersedes a question the user can still answer

- **Status**: Accepted
- **Date**: 2026-09-23

## Context

ADR 0082 made the message router supersede an open question whenever a
coding-agent follow-up could not be its answer. It named two such shapes: an
agent-driven message, and a message landing on an overtaken question. Both
emitted `CodingAgentPromptSent`, which is in `QUESTION_OVERTAKEN_EVENT_TYPES`.
So the follow-up killed the card, and only a supersede could release the parked
agent.

A parent agent then asked the user a question. A child thread finished 39
seconds later. Its `ChildThreadCompleted` wake reached the router, and the
router superseded the question, with the child as the actor. The user never got
to answer it. The card read "Replaced by your next message", although the user
had sent nothing.

A child's completion is an engine re-entry, not a message. It is forwarded as
`AgentInputKind::ReentryFromEngine`, which emits no `CodingAgentPromptSent`.
`ChildThreadCompleted` is not in the overtaken set and moves no thread status.
So the card was still live, and the deadlock ADR 0082 prevents could not occur.

## Decision

An engine re-entry (`PreEmittedOrigin::EngineReentry` or `WaitReentry`) leaves
an open question alone when the question is still active and the thread's agent
session is live. Every other non-answering follow-up still supersedes, exactly
as ADR 0082 says. The rule is `follow_up_keeps_open_question` in
`engine/chat/process_helpers.rs`.

## Rationale

ADR 0082's invariant is that a thread is never left holding a question nothing
can answer. Here the user can still answer it. The re-entry waits in the agent's
input queue and is read after the answer, so nothing is lost by keeping the
question. Superseding it lost the question and gained nothing.

Both conditions are load-bearing:

- **Active.** An overtaken question has dead buttons. Nobody can answer it, so
  the supersede is still the only thing that releases the agent.
- **Live session.** With no live session, nothing is parked on the question.
  The re-entry starts a fresh session, whose first step overtakes the card, so
  the question would die unanswered anyway.

## Consequences

- A child report or an event-wait delivery arriving mid-question no longer
  destroys the question. The agent reads the report after the user answers.
- A parent agent's follow-up into a child that is parked on a question still
  supersedes it. That follow-up emits `CodingAgentPromptSent` and would recreate
  the ADR 0082 deadlock. Keeping the question there needs the follow-up held
  until the question resolves, which is a separate change.
- A human message is unaffected. One that can answer the question answers it.
  One on an overtaken question supersedes it.
- The chat lane is unaffected. It never superseded.

## Alternatives considered

**Never let any agent-mode message supersede.** Covers the parent follow-up
too, but brings back the ADR 0082 deadlock for it: its prompt kills the card
while the agent stays parked. That needs a hold mechanism first.

**Suppress the child callback while the parent has an open question.** Moves
the decision into `parent_callback.rs`, which a concurrent change is already
reworking for stopped and waiting children. It also delays a report the parent
may need, where queueing it behind the answer delays nothing that matters.
