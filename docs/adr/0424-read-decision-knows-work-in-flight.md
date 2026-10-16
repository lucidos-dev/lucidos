# 0424: The read decision knows when work is still in flight, and a read request lists during a live event wait

- **Status**: Accepted
- **Date**: 2026-10-10
- **Amends**: [0409](0409-review-lists-read-requests.md), [0417](0417-turn-end-gate-enforces-valid-states.md)

## Context

A nightly trigger orchestrates several sub-threads. It wakes on each
sub-thread's report and ends each wake with a short progress reply. The
turn-end gate forced a read decision on every one of them, and its auxiliary
model said yes each time. So every step put the thread in Review with the
unread dot, while only the ending was worth reading.

The model saw the opening message and the reply, and nothing else. A progress
report with no sign that work remained read like a finished run's findings.

## Decision

- **The gate tells its model when the thread is still in flight.** That means
  sub-threads still mid-turn or idling on their own wait, or a live event wait
  of its own: the facts behind the drawer's Waiting dot. A closing note names
  them, and the prompt says a progress report is then a no.
- **The model keeps the judgement.** A yes is for what the user should read
  before the work finishes, such as a failure, a finding or a decision. The
  turn that ends with nothing in flight is judged as before.
- **The `request_read` tool description says the same**, for a chat agent that
  decides by itself. A coding agent's read rule already says yes only for an
  answer, so it gains no sentence: every session prompt is at its size ceiling.
- **A read request lists in Review whatever the thread still waits on.** A
  live event wait no longer hides it from Review, the dot or the seen tracker.
  It still never lists while a turn runs or a question waits.

## Rationale

The user's rule: only the ending belongs in Review, but a step that holds
something to read should say so. A blanket rule cannot tell those apart; the
model can, once it knows the work is unfinished.

A live event wait hid a read request only because the same check guards a
change: its branch is still being worked on, so Apply must wait. A read
request has no Apply. Hiding it would swallow exactly the mid-flight yes this
decision keeps, and seeing the reply could not clear it either.

## Consequences

- A progress turn costs the same model call as before. The note adds a
  sentence to its prompt.
- A thread can sit in Review while its sub-threads run, when its agent asked.
  It leaves In flight for that time, since In flight reads the dot.
- A change during a live wait still reads Waiting and stays out of Review.
- `readRequestStanding` is the one client definition Review and the seen
  tracker share.

## Alternatives considered

- **An in-flight thread holds no read request at all**, enforced by a
  database trigger like ADR 0421. Rejected by the user: a step worth reading
  would be silenced.
- **Keep hiding a read request during a live wait.** Rejected: an orchestrator
  that waits on events would never surface a mid-flight finding until the wait
  ended, by which time the reply is stale.
- **A rule in the nightly knowhow only.** Rejected: the gate forced the
  decision, not the orchestrator, and every orchestrating thread has the
  same shape.
