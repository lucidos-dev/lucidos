# 0256: An agent-sent message waits behind a coding agent's open question instead of superseding it

- **Status**: Accepted
- **Date**: 2026-09-23

## Context

ADR 0082 made the message router supersede an open question on a coding-agent
thread whenever a follow-up could not be its answer. An agent-sent message can
never be the answer, because the answer route requires a human. So a parent's
follow-up into a child parked on the user's question resolved that question as
`Superseded`. The parent's message replaced the user's answer.

The engine already promised the opposite. `child_follow_up` classified such a
child as `WaitingForUserAnswer` and told the parent: "It will not read this
until a human answers." A Lucidos Agent child kept the promise, because its lane
queues the message as an injection. A coding-agent child broke it.

ADR 0255 fixed the same shape for engine re-entries, which never disable the
card. An agent-sent message does disable it: the router forwards it as a user
input, and its `CodingAgentPromptSent` is in the overtaken set. So simply
keeping the question would bring back the ADR 0082 deadlock. The message has to
wait instead.

## Decision

On a coding-agent thread, a fresh agent-sent message is **held** while a human
owes the thread a reply. That means an answerable question, a pending permission
card, or older held messages still waiting. It is recorded as `MessageHeld` and
not forwarded. The parent's follow-up result reports the new delivery `held`.

Any answer except Cancel releases every held message, oldest first, and so does
an Allow or Deny on the card. So do the user's next message and Continue. A
Cancel keeps them held.

Each release emits `HeldMessageReleased` and then the ordinary `MessageReceived`,
and forwards the message to the agent. A unique index allows one release per
held message. The rule is `message_is_held` in `engine/chat/held_messages.rs`.

The release is guaranteed to reach the agent. The session keeps its subprocess
until the agent reports the message read, per ADR 0268. The transcript shows
the delivered message as "Sent", then "Read".

## Rationale

**A new event, not `MessageReceived`.** `MessageReceived` sets the thread to
`running` in the projection. Recorded that way, a held message would move a
question-parked child out of `waiting_for_user_answer`. `MessageHeld` moves no
status, and it is the durable record, so a hold survives a restart.

**Cancel keeps the hold.** The user decided this. Cancel means stop, so a
parent's message must not restart the child the user just stopped.

**Older held messages hold newer ones.** After a Cancel no question is open.
Without this rule a newer agent message would overtake an older held one and
restart the child. With it, send order is kept and the Cancel is respected.

**Emit the release before delivering.** A crash between the two leaves a message
that is visible but undelivered, which is loud. The reverse order can deliver a
message twice, which is silent.

**Codex waits for the turn to end.** A Codex agent interrupts a running turn for
any new input. So a release waits for the turn the answer resumed to finish.
Claude Code needs no wait. It reads a mid-turn input at its next tool result, or
as its next turn when none follows. The session stays up for that turn because
the input is owed until Claude Code replays it (ADR 0268). Before ADR 0268 the
session exited at the answered turn's `Result`, and the released message was
lost.

## Consequences

- A parent's follow-up, or any other agent-sent message, no longer destroys the
  user's question. The transcript shows it as "Held until you reply".
- A human message is unaffected. One that can answer the question answers it.
  One on an overtaken question supersedes it, per ADR 0082.
- An agent message on an overtaken question still supersedes it. Nothing is
  parked on a dead card, so there is nothing to wait for.
- A release that a restart interrupts leaves the message held and visible. The
  user's next message or Continue releases it. No boot sweep delivers it,
  because a restart must not start work on its own.
- Held messages on an archived or discarded thread are never delivered.
- The Lucidos Agent lane is unchanged. It already queues these messages.
- A pending permission card holds agent messages too. An agent message used to
  deny the card for the user ("Superseded by a new message"). A request the user
  was about to approve was silently cancelled. A human message still supersedes
  the card, as before.

## Alternatives considered

**Forward the message, and keep the question.** Claude Code would read it only
after the answer anyway. Rejected: the forward emits `CodingAgentPromptSent`,
which disables the card and the typed-answer route. That is the ADR 0082
deadlock.

**Hold with a flag on `MessageReceived`.** One event instead of two. Rejected:
it puts a hold branch in the projection of the most common event. Every reader
of `MessageReceived` would also have to learn that some were never delivered.

**Deliver on Cancel too.** Simpler, and nothing waits on a later reply. Rejected
by the user: Cancel means stop.
