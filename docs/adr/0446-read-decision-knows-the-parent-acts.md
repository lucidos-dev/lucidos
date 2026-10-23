# 0446: The read decision knows when a sub-thread's report wakes its parent agent

- **Status**: Accepted
- **Date**: 2026-10-12
- **Amends**: [0417](0417-turn-end-gate-enforces-valid-states.md), [0434](0434-sub-threads-wake-parents-only-when-needed.md)

## Context

A coding-agent session started eight chat sub-threads overnight, one per
archive run. None decided its own read, so the turn-end gate forced the
decision. The judge said yes on all eight, including five clean reports of
"0 missing, nothing to do". The user woke to eight unread threads.

Every one of those reports also woke the parent session, which read it and
acted on it. The user reads the parent. So each read request only duplicated
what the parent already handled.

The two turn-end decisions each assumed the other covered the user:

- The read judge saw "a prompt from another agent" and a reply full of counts.
  Its yes criteria name "findings" and "what a scheduled run found". Nothing
  told it the reply was a report to a parent that acts on it.
- The wake judge says no when "the child's findings already reach the user
  from the child's own thread". That holds only while the read decision
  surfaces them.

Plan: `docs/plans/2026-10-12-read-decision-knows-the-parent-acts.md`.

## Decision

A forced read decision on a turn the user did not open learns whether the
turn's report wakes the parent. When it does, the prompt ends with a closing
note saying the parent agent acts on the reply now. The instructions say a
routine result for that agent is not worth the user's read. A failure, a gap
the user must know about, or something only the user can do or decide still
is. The instructions name a payment, a sign-in and an approval, and say the
parent seeing the reply does not make it the parent's to settle.

- **"Wakes the parent" has one definition**: `Delivery::for_report`, the rule
  the fan-in delivers by. `OwedReport::wakes_parent` reads it for a finished
  turn. A cut-short turn always wakes. A turn that ends holding a live event
  wait wakes nothing yet: the fan-in holds its card until the wait ends, so
  that turn gets the in-flight note and no parent note.
- **The wake decision settles first** when the read decision is forced on such
  a turn. Every other turn still runs the two calls at once, so only these
  turns pay one extra call of latency.
- **A turn the user opened, or resumed with an answer, gets no note.** The
  reply speaks to the user there, whoever else reads it.
- **A filed report gets no note.** The parent does not act on it now, so the
  child's own read decision still has to surface it.
- An agent's own `request_read` choice is never touched.

## Rationale

The judge has the last word because a parent can end its own turn with a terse
reply. A must-see failure in the child then still reaches Review.

Reusing the fan-in's delivery rule keeps the note true. A second copy of "which
reports wake" could drift, and the note would then claim an action the parent
never takes.

The closing note follows ADR 0424's shape. That note taught the judge that a
progress report on work in flight is not worth reading, and it measured well.

## Consequences

- Fewer read requests from orchestrated sub-threads. Their results reach the
  user through the parent's reply and its own read decision.
- A forced read on such a turn waits for the wake call. That costs one
  auxiliary call of latency, before `ResponseGenerated`.
- The labelled eval set gains turns of this shape, and `lucidos-eval
  read-decision` reads a `report_wakes_parent` field.

## Alternatives considered

- **Skip the read call when the report wakes the parent**, and record a no.
  Rejected: cheaper and predictable, but a failure the parent's reply drops
  would never reach Review.
- **Tell the judge only that the thread has a parent.** Rejected: a filed
  report is never acted on now, so the note would be false there. Home's
  children mostly file.
- **Strengthen the read criteria alone.** Rejected: the judge cannot tell a
  report its parent consumes from one only the user reads without being told.
