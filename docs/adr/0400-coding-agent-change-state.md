# 0400: A coding-agent thread's change state is none, unproposed with a reason, or proposed; an unfinished turn never proposes

- **Status**: Accepted
- **Date**: 2026-10-09

## Context

ADR 0397 made a turn end hold a change the plan floor would refuse. The work
stayed on the branch, but nothing named that state. `thread_summaries` carried
two booleans, `coding_agent_has_diff` and `coding_agent_proposed`. "Has a diff,
not proposed" was the only trace of a hold. The same pair also meant a running
turn, a live event wait, an external repo or set-aside work. No event announced
a hold, the UI showed nothing, and the nightly orchestrator could not see held
work.

Settling the model exposed a second mismatch. ADR 0346 and ADR 0328 make an
unfinished turn propose its branch, flagged `incomplete`. So a proposal could be
work nobody should apply yet, and a third boolean,
`coding_agent_incomplete`, said so.

## Decision

A coding-agent thread has one *change state*:

- `none`: no work on the branch;
- `unproposed { reason }`: work on the branch that is not a change;
- `proposed { requires_restart }`: a pending change exists.

The reason is one of `plan_missing`, `plan_awaiting_approval`, `outside_bound`
(from `PlanHold`) or `turn_incomplete`. It is empty when no turn end withheld
the work.

An unfinished turn never proposes: a Stop, a failure, an abort or a cut-off. A
turn end that leaves work unproposed announces it with `ProposalWithheld`. Apply
acts only on a proposal. A change whose work became unfinished is *withdrawn*
(`ChangeWithdrawn`) back to unproposed work, with its branch untouched.

## Rationale

- **One value answers "is there unapplied work".** A consumer checks the kind;
  the reason answers why. Two booleans made the consumer rebuild the state and
  allowed combinations nobody meant.
- **A proposal means ready to apply.** An incomplete proposal asked the user to
  judge partial work, and needed its own card face, confirm and blocker. As a
  reason for unproposed work it needs none of them.
- **Only the floor's and the turn's verdicts are typed.** Running, a live wait,
  an external repo and set aside are already readable from other columns, so
  the reason does not restate them.
- **Withdrawing keeps Apply honest.** A stopped turn that commits on top of a
  pending change would otherwise let Apply merge its partial commits.

## Consequences

- Stopped work loses its Apply until a finished turn proposes it. Continue is
  the way on. This supersedes ADR 0328's "a Stop proposes" and the proposing
  half of ADR 0346.
- Stopped work no longer blocks Archive. The archive net sets it aside, as it
  does for any unproposed work. An agent's archive request no longer waits on
  withheld work either.
- Work held by a live event wait loses its immediate Apply. Apply on settle
  still waits for a real proposal.
- Bring back of an incomplete set-aside row withdraws it and decides again:
  propose, or withhold with the real reason.
- The first boot withdraws every pending incomplete change, so no proposal is
  ever incomplete afterwards. The branch keeps its commits.
- A `NULL` reason on a running or waiting thread is expected, not a gap.

## Alternatives considered

- **Keep the booleans and add a reason column.** Rejected by the user: it adds
  a third flag and keeps the impossible combinations.
- **Keep incomplete as a kind of proposal**, `proposed { incomplete }`. The user
  ruled it makes no sense to propose unfinished work.
- **Typed reasons for every cause**, including running and live wait. They
  would restate columns that already hold the answer.
- **Name the event `ChangeHeld`.** "Held" already means the event-wait hold of
  ADR 0395, so a reader would confuse the two.
