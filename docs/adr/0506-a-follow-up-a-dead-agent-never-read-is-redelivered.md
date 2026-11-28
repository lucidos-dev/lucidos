# 0506: A coding-agent follow-up a dead process never read is redelivered to the next session, and removable without the agent

- **Status**: Accepted
- **Date**: 2026-10-17

## Context

A follow-up sent to a running Claude Code session goes to its stdin at once.
Claude Code holds it in its input queue until it reads it (ADR 0268). That
queue is process memory. A process that dies first takes the follow-up with it.
A card parked for hours in the `AskUserQuestion` hook made this likely: the
engine restarted, the process died, and the resume carried only the answer.

The bubble then said "Queued" for good, which was true. Remove failed with a
409 "already read", which was not. `begin_withdraw` asked only the live
session's ledger, and an input forwarded to an earlier process is absent there.

## Decision

A human follow-up is a *stranded follow-up* when a `CodingAgentPromptSent`
names it, and no `CodingAgentInputRead` or `QueuedMessageRemoved` does. A
starting coding-agent session forwards each stranded follow-up of the previous
session again. Each goes as its own input behind the first prompt, with a note
that it is late. A Remove for an input no live process holds records the
tombstone from the store: 409 if the agent ever read it, 200 if not.

This extends ADR 0236 to the coding-agent lane, and narrows ADR 0323's "Claude
Code decides" to inputs a live Claude Code process holds.

## Rationale

- **The event store already knows.** Every marker the predicate reads is
  written today, so the fix adds no state. It is the same move as ADR 0236.
- **Only the agent writes a read.** A synthetic `CodingAgentInputRead` would
  clear the bubble and hide a message nobody read. The redelivery's own read
  names the original message, so the bubble clears truthfully.
- **Its own input keeps it withdrawable.** A fresh stdin uuid and its own
  ledger entry let `cancel_async_message` take it back until read. Coalesced
  into the first prompt, it could never be withdrawn (ADR 0323).
- **A dead process cannot read.** So with no live process holding the input,
  the store's answer is final, and Claude Code has nothing to decide.

## Consequences

- **The window is the previous session only.** Older stranded rows stay as
  they are, as in the Lucidos Agent's fix. A cache keep-alive's session does
  not count, or its beat every few minutes would hide the message.
- **A redelivery lost again is caught again**, since the session that lost it
  forwarded it too.
- **The redelivery is marked unforwarded before it enters the channel**, so a
  withdraw waits for its forward rather than overtaking it.
- **Remove on an idle thread claims the thread first.** No session can then
  start and forward the message while the tombstone is written. A claim
  refused by a live run hands the request to that run's loop.
- **Only a forwarded message is taken back from the store.** One no session
  ever forwarded may be waiting on a spawn's claim, which would deliver it over
  a tombstone. So Remove refuses it. A message lost before any forward, as
  in a crash with it still in the session's channel, therefore stays queued.
- **A live Codex session still refuses at once**, as ADR 0323 records. An idle
  Codex thread takes the claim path like any other.
- **A spawn's own first prompt is not redelivered.** A session that dies
  before it reads it takes the start-failure path, which carries the message.

## Alternatives considered

- **Write a synthetic read when the bubble looks stuck.** The user's first
  idea. It hides a lost message, and the user rejected it once the transcript
  showed the agent never read it.
- **Make Remove honest only.** The bubble could then leave, but the message
  stays lost unless the user notices and resends it.
- **Coalesce the stranded messages into the first prompt.** One write fewer,
  but a coalesced write cannot be taken back one message at a time.
- **Queue the redelivery behind input sent during the startup.** Simpler, but
  the agent would then answer a newer message before the older one it may
  build on. The session puts the startup input back behind it instead.
- **Redeliver every stranded row in the thread's history.** It would replay
  messages weeks old into a conversation that has moved on, answering nobody.
