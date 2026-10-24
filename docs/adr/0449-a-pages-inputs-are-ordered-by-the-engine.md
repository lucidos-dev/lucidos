# 0449: A page's inputs to a thread are ordered by the engine: a client input sequence, checked under the thread append lock at admission

- **Status**: Accepted
- **Date**: 2026-10-12

## Context

The client kept a device's messages in order by sending one per thread at a
time (`store/actions/sendChain.ts`). The engine recorded them in arrival order,
and the request carried no ordering data. So the client chain was the only
thing that ordered sends, and it trusted two things that were not true:

- **A `200` meant the message was recorded.** It did only for a chat follow-up
  on an existing thread with no open question. A new thread, a typed answer and
  a coding-agent follow-up were recorded after the ack.
- **A send still unsettled after 55 s would never arrive.** A send that was
  behaving could take about 70 s. A request the client gave up on could still
  be recorded by a detached task.

Separately, the duplicate guard recorded a message's id before the message. A
dropped request between the two lost the message, and every retry was acked as
a duplicate.

The evidence is in `docs/plans/2026-10-12-sends-ordered-by-the-engine.md`.

## Decision

Every input a page sends to a thread carries the page's *input chain* id and an
*input sequence*. The engine admits an input before the ack, under the thread
append lock (ADR 0360). It admits it only if its sequence is past the last one
admitted from that chain on that thread. Admission also joins the thread's
*input order*, and every message records itself only after the input
admitted before it.

## Rationale

**The engine refuses the overtaken input, not its successor.** When a send gave
up, the client cannot know whether it will ever land. A successor refused until
it does is stranded, or needs a resend once the predecessor settles. A late
input refused instead needs nothing new: the client already shows it as Not
sent.

**The order is fixed at admission, so the ack can mean it.** Some inputs must
record late: a coding-agent follow-up waits for a Codex redirect boundary (ADR
0281). Fixing the order at admission, and recording in that order, makes the
`200` mean "your place is held" on every path.

**The append lock makes the check and the join one step.** Two admissions on a
thread serialize on the lock, so the order they join is the order the check
accepted them in.

**Admission and dispatch run as one detached task.** A dropped request can then
only drop its own wait. It can no longer split an admitted input from the work
that records it.

## Consequences

- The client chain still sends one POST per thread at a time. It now only
  paces: without it a quick second press would overtake the first and get it
  refused. Its 55 s hold cap is gone, since every send settles on its own.
- A manual Retry of a Not sent message takes a fresh sequence and lands at the
  tail. Quiet retries resend the same sequence, so they stay the same input.
- `FollowUpOrder` became `InputOrder`, covering every input the chat and answer
  endpoints admit. A chat follow-up with an input ahead of it records in its
  spawned turn instead of before the ack.
- A card answer is checked and placed, but records at once. A running turn may
  be blocked on that very question, so waiting behind an earlier input could
  deadlock it.
- Admission runs before the duplicate check. A refused straggler then never
  claims an event id that a Retry of the same message gets acked against.
- A send to a thread whose first input is admitted but not yet recorded is no
  longer refused as unknown.
- Inputs without a chain (agent mode, the CLI, another workspace, a stale
  client) are never refused and write no chain row. They still join the input
  order.
- Order across pages, and so across devices and tabs, is admission order. No
  shared clock exists to do better.

## Alternatives considered

**A `prev_event_id` precondition that refuses a send whose predecessor has not
landed.** Rejected for the stranding above. A client that gave up on its
predecessor has no correct value for `prev`.

**Ordering by the client's wall clock.** Rejected, as in the 2026-07-30 plan: a
device clock is untrusted, and it would not decide which message starts a turn.

**Keep the hold cap as a liveness net.** Rejected. Once the engine refuses a
late input the cap orders nothing, and every await before the POST already has
its own deadline.

**Record every input before the ack.** Rejected: the Codex redirect contract
needs the coding-agent follow-up to record after the interrupted turn's
terminal.
