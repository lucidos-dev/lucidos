# 0281: Coding-agent follow-ups keep their sent order through a per-thread handoff joined before the ack

- **Status**: Accepted
- **Date**: 2026-09-25

## Context

The chat handler acks a coding-agent follow-up before recording it. Each
spawned turn then waits for the agent session on its own 100 ms polling timer.
Once the session is live, it records and delivers the message. Two follow-ups
sent while a session started could therefore be recorded and delivered in
either order. A nightly e2e run caught one: the second follow-up took the lower
sequence and reached the agent first.

The chat lane closed the same race by recording the message before the ack
(`docs/plans/2026-07-30-serialize-chat-sends-per-thread.md`). That path skips
coding agents on purpose. A Codex redirect must let the interrupted turn's
terminal take its sequence before the follow-up's `MessageReceived`.

## Decision

The handler joins a per-thread chain before its ack (`FollowUpOrder`, in
`engine/chat/follow_up_order.rs`). Each follow-up waits for the one before it,
then records and routes its message, then releases the next.

## Rationale

**The ack is the only point the client orders.** The frontend sends one message
at a time per thread and waits for each ack. So the order the handler sees is
the order the reader sent. Joining never waits, so the ack stays as fast.

**It orders follow-ups, and nothing else.** The redirect wait, the session
wait and the emit stay where they are. Only follow-ups to one thread are
ordered against each other, so the Codex redirect contract holds unchanged.

**It covers both backends.** Claude Code and Codex share this path.

**A dead follow-up cannot stall or reorder the rest.** Dropping a turn releases
its successor, but only after its own predecessor is done. A task that dies
while it waits therefore leaves the order intact.

## Consequences

- Follow-ups to a starting session land in sent order on both backends.
- A follow-up waits for the one before it to be recorded and routed. When that
  one falls to the slow path and queues behind a running turn, the next waits
  with it. That is the order the reader sent them in.
- A follow-up waits for its turn before it takes a capacity slot (ADR 0008).
  A later follow-up therefore never holds a slot idle behind an earlier one.
- Callers other than the chat handler (child wakes, held-message releases,
  event-wait deliveries) do not join the chain. None of them promised an order
  against the reader's sends.

## Alternatives considered

**Record Claude Code follow-ups before the ack, as the chat lane does.** A
smaller diff, and the narrower of the two options offered. It leaves Codex
follow-ups free to reorder. The Claude Code dispatch would also have to skip its
own emit, and `MessageReceived` would land before `SessionStarted` on a
starting session. The maintainer chose the thorough fix.

**Replace the polling wait with a notification.** Waking every waiter at once
decides nothing about their order, so it would not fix the race.
