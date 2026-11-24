# 0706: A queued chat message is taken back or consumed, never both: the event bus decides

- **Status**: Accepted
- **Date**: 2026-11-15

## Context

On a Lucidos Agent thread, `POST /api/v1/chat/queued-message/remove` wrote
`QueuedMessageRemoved` and answered 200 whenever no `PromptInjected` named the
message yet. Only the round-boundary drain and the orphan chain read that
tombstone. The Thread Queue executor, the admission loop's fresh-turn fallback
and the question router did not.

So a removed message could still run as its own turn, or become the answer to
an open card. The client trusted the 200 and put the text back in the composer,
and the user's resend was the second copy. ADR 0323 had already settled the
same problem on the coding-agent lane: there the tombstone follows only the
holder's confirmation.

## Decision

A human message is either taken back or consumed, never both. The event bus
decides it in the emit's Validate phase, under the thread append lock
(`engine/consumption_claim.rs`). We call this the *consumption claim*.

- Consumed means one of four markers: a `PromptInjected` naming the message, a
  `ChatTurnSettingsResolved` anchored on it, or a `UserQuestionAnswered` or
  `UserOnlyMessagePosted` with its id. User-only delivery uses the same claim,
  so Stop cannot restore a message that still reaches the other users.
- Once a `QueuedMessageRemoved` names a message, the bus refuses its
  `MessageReceived` and each of those markers.
- On a chat thread, the bus refuses a `QueuedMessageRemoved` once the message
  is consumed. The endpoint maps that to 409 `already_read`.
- The coding-agent lane is never refused here. Its holder already confirmed the
  withdrawal (ADR 0323).

The refusal reuses `LifecycleViolation` with a named reason. Each caller reads
it through `is_removed_refusal` and drops the message quietly: nothing
recorded, no failure shown. A removal also releases the message's place in the
input order, so its successor stops waiting.

## Rationale

The claim lives where every consumer already passes. Each delivery path writes
one of the three markers through the bus. So one rule covers all of them,
including paths added later. The check and the marker commit in one
transaction, so a race between a removal and a delivery has exactly one winner.
Nothing about the claim lives in memory, so the engine stays restartable.

A turn starter needs no special case. Its turn stamps
`ChatTurnSettingsResolved` right after recording it. A removal that lands in
between refuses that stamp, and the turn never runs. We rejected a rule that
reads every message recorded without an anchor as a starter. It also catches a
follow-up that arrived before the live turn stamped its anchor. That follow-up
would then answer "already read" while it still waited.

## Consequences

- A 200 from the removal endpoint now means the message never reaches the
  agent. The client can safely restore its text for a resend.
- Every consumer must treat the removed refusal as a quiet drop, not a failure.
  `withdrawn_or` in `chat/process/run.rs`, the drain filter, the orphan batch
  and the question router do this today.
- The endpoint no longer pre-checks `PromptInjected` itself. There is one
  definition of consumed.
- Each consumer emit pays one indexed lookup for a tombstone.

## Alternatives considered

- **Check the tombstone in each consumer.** This is what we had, for two of
  five paths. Each new path must remember the check, and a check outside the
  append lock races the removal.
- **An in-memory claim map.** Fast, but lost on restart, and it splits the
  truth between memory and the event store (ADR 0236).
- **A queue table with a status column.** ADR 0236 already rejected a separate
  queue store. The event store holds queue membership.
