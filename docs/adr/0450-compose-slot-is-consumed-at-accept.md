# 0450: A submission consumes the compose slot once, when the engine accepts it, and clears only the content it submitted

- **Status**: Accepted
- **Date**: 2026-10-12

## Context

Each thread has one stored compose draft and a *compose epoch* that counts
submissions. A compose PUT carries the epoch it was composed under, and the
engine refuses a write from before a submission.

Three projection arms consumed the slot: `MessageReceived`, the coding-agent
`SessionStarted` and `ContinuationStarted`, and `UserQuestionAnswered`. Each
cleared the stored draft and raised the epoch when its event was written. On a
coding-agent dispatch and on a question reroute that happens after the chat ack.
A coding-agent send consumed twice, once per arm.

So a draft typed after the send could be stored, at the old epoch, before the
arm ran. The arm then wiped it. An agent's `MessageReceived` into the thread
wiped the user's draft the same way. The client hid the loss by keeping such a
draft on screen only, guarded by `Date.now()` stamps and a supersede rule.

The compose draft now follows a server revision
(`docs/plans/2026-10-12-compose-draft-follows-a-server-revision.md`). A client
that shows the server's state faithfully would show that loss, so the engine
side had to change first.

## Decision

The engine consumes the slot when it accepts a composer submission, before it
answers: in `chat_submit` for a person's send to an existing thread, and in the
answer-question endpoint. It always raises the epoch, and always clears the
draft's mode and selection, which the send carried. The stored text and images
clear only when they are the submitted content or an earlier state of it. That
covers a Send that beat the last debounced write. The chat ack carries the compose
state the consumption left, at its revision, and both paths broadcast it. No
projection arm consumes for a composer submission.

The voice call's promotion keeps its own arm. That consumption is not a composer
submission.

## Rationale

The accept is the one moment both sides agree on. The client learns the new
epoch in the same response, so its next write lands after the consumption by
construction. A later event can no longer reach back and clear a draft written
in between.

Clearing only the submitted content makes "consume" mean what the user did. A
retry of an unsent message after a reload, or a reply rerouted to a question,
leaves a newer draft alone. Raising the epoch unconditionally still refuses a
stalled write of the sent text.

## Consequences

- The chat ack carries a compose report. The answer endpoint, whose client
  reads only success, announces its report over SSE.
- An agent's message into a thread leaves the user's draft alone.
- The engine tests that pinned arm consumption moved to the accept path.
- The client no longer needs the supersede rule to guess whether a peer sent
  its draft. A report at a newer revision says so.

## Alternatives considered

- **Keep consumption in the arms, and mark an event "already consumed".**
  Rejected: the ack still could not report the new epoch on the paths that emit
  after it, and every arm would carry a second rule.
- **Hold client writes after a send until the epoch is seen to move.** Rejected:
  a send whose event never lands would hold the draft unsynced with nothing to
  release it.
- **Clear unconditionally at accept.** Rejected: a retried or rerouted send
  would wipe a draft typed after the original.
