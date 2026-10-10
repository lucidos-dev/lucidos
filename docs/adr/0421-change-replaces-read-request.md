# 0421: A change that already lists the thread replaces a read request: a trigger keeps them apart, and the turn-end gate decides no

- **Status**: Accepted
- **Date**: 2026-10-10
- **Amends**: [0409](0409-review-lists-read-requests.md), [0417](0417-turn-end-gate-enforces-valid-states.md)

## Context

A coding agent ended a turn without a read decision. The turn-end gate forced
one, just after the turn's change was proposed, and the auxiliary model said
yes. The user applied the change from Review. The read request stayed, so the
thread sat in Review with the unread dot after its work had landed.

A proposed change and a read request ask the user for the same thing, a look.
Only the change has a clean end, Apply or Discard. A request beside it outlives
it.

## Decision

- **Two change states replace a read request**, because each already lists the
  thread in the Ongoing grouping:
  - a proposed change, in Review;
  - work held for a missing harden, in Blocked.
- **A Postgres trigger on `thread_summaries` owns the rule.** It sets
  `read_requested` to FALSE whenever the SQL function
  `change_replaces_read_request` says one of those two states holds. A request
  made earlier in the turn drops when the change arrives, and a request made
  while one is held sets nothing.
- **The turn-end gate reads the same function**, and decides no without a model
  call when it holds. It records `ThreadReadNotRequested` in the engine's name.
- **Other held work keeps its request**: a missing plan, a plan awaiting
  approval, a turn left unfinished, work outside the bound, or unproposed work
  with no reason.

## Rationale

The user's rule: a proposed change, or a thread otherwise blocked, means no read
request. Blocked is a drawer group, so the rule follows what the drawer lists.

Other held work lists the thread in neither Review nor Blocked. Dropping its
request would hide the reply from every Ongoing group, the silence ADR 0417
rejected.

A trigger, as for `summary_version` (ADR 0329), because several writers move
the change state. `ChangeProposed`, `ProposalWithheld` and the idle all do,
and the post-commit hook and the session seed do so with no event at all. A
rule in each writer is one a future writer forgets. Two of those writers also
share SQL with the archive, which sets `read_requested` itself, so a clause
there would assign the column twice.

One SQL function defines the two states, so the trigger and the gate cannot
disagree. The gate decides no, rather than letting the trigger drop a yes, so
the recorded event says what the drawer shows. It also spends no model call on
an answer that is already known.

## Consequences

- Apply and Discard need no clear of their own. The request could not exist
  while the change was pending.
- A Discard does not bring back a request the change replaced.
- Chat and trigger threads are unaffected: their change state stays `none`.
- An agent's own `request-read yes` on a thread in one of the two states
  records the event and projects nothing.
- If Review or Blocked starts listing another held state, extend
  `change_replaces_read_request` in a new migration.

## Alternatives considered

- **Every held state replaces the request.** Rejected in review: a missing plan
  or an unfinished turn lists the thread nowhere, so its reply would vanish.
- **Clear the request on `ChangeApplied` and `ChangeDiscarded`.** Rejected:
  until then the thread shows both a change and an unread dot. Every other way
  out of a held change would also need the same clear.
- **The rule in each writer, no migration.** Rejected: five writers, two with
  no event, and a duplicate-assignment clash with the archive's SQL.
- **A CHECK constraint.** Rejected: a writer that broke it would fail its
  event's whole transaction, losing the event rather than the flag.
- **Tell the auxiliary model about the change.** Rejected: its prompt already
  says a reply that only reports a ready change is a no, and it still said
  yes. A fact the engine knows needs no judgement.
