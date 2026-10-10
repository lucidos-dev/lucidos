# 0397: Turn end holds a change the plan floor would refuse, and nudges the agent

- **Status**: Accepted
- **Date**: 2026-10-09

## Context

Only Apply checked the plan marker. Turn end proposed every Lucidos-source
branch, so a branch with no marker still got an Apply button, and Apply then
refused it.

The repeating shape: a change applies, and Apply consumes the marker. The same
session keeps working on the same branch. A Bash edit (`python3`, `sed`) never
passes the `cc-plan-gate` hook, which sees only `Edit` and `Write`. Turn end
then proposes the new commits with no marker. Codex has no hooks at all, so
there the only check was Apply.

The user asked for no proposal of an invalid state.

## Decision

Propose and Apply ask one function, `change_ops::PlanFloor`. A proposal also
runs Apply's bounded security-fix check on the files it would propose. When
either holds a branch, `propose_change` proposes nothing and returns
`HeldForPlan`. A live
session then gets one engine message per branch HEAD, naming how to set the
marker. Its next turn end proposes the change.

## Rationale

- **One decision cannot drift.** Two copies of the floor are what let the
  propose path and Apply disagree.
- **The chokepoint covers every path.** The live idle, session completion,
  `apply_now`, stale-session recovery and stopped work all reach
  `propose_change`. The boot held-back sweep emits directly, so it asks the same
  function.
- **The engine can fix what it detects.** It knows the marker is missing at
  turn end, and the agent can set one in the same session. A nudge keeps the
  work moving without the user typing "no plan again".
- **The commits stay on the branch.** Holding a proposal loses nothing: the
  next satisfying turn end proposes it, and the archive net still sets aside
  unproposed work.

## Consequences

- Work after an Apply needs a new plan decision, as before. The agent now hears
  that at turn end rather than the user hearing it at Apply.
- `apply_now` gains the full plan floor, checked before hardening. It used to
  check only the bounded security-fix lane.
- A held branch shows no change card until the agent sets a marker. The nudge's
  exchange in the thread says why.
- An agent that ignores the nudge ends its turn. The next nudge comes only with
  new commits, so it cannot loop.
- The Apply floor stays as the backstop, for rows proposed before this gate.
- One edge stays open. A new plan recorded mid-thread puts the marker back to
  `proposed`. A change already pending then keeps its card, and new commits no
  longer re-sync it. Apply refuses it with the awaiting-approval
  message, and approving the plan clears it.
- ADR 0328's "a Stop proposes, so stopped work keeps an Apply" now holds only
  where the plan floor clears the branch.
- A plan-only branch is the one exception. When every landing file is under
  `docs/plans/`, a `proposed` marker clears the floor, so the plan can land as
  a record. Apply keeps that marker, and `lucidos planned approve` flips it and
  the plan file's `Status` line together.

## Alternatives considered

- **Keep the marker across Apply.** Follow-up work on the same thread would
  pass without a decision. Rejected: it weakens the gate the user wants, and a
  thread can drift into an unrelated task.
- **Catch file writes in `cc-bash-guard`.** Bash cannot be parsed reliably, and
  Codex has no hook to put it in. The propose gate makes the bypass harmless.
- **Propose the change marked "needs a plan", with no Apply.** That is a new
  card state for work that is not ready to review, and the user still has to
  chase the agent. Rejected for the hold and the nudge.
- **Block the Claude Code Stop hook instead.** It covers Claude Code only. The
  engine check covers both agents in one place.
