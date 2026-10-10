# 0416: Unhardened Lucidos-source work is never proposed; a plan-only branch never hardens; Ready means apply-ready

- **Status**: Accepted
- **Date**: 2026-10-10

## Context

A branch holding one proposed plan file cleared the plan floor as
`PlanFloor::PlanOnly` (ADR 0397). Apply then failed with "Hardening did not
complete (no marker recorded)". The hardening gate skipped app threads only, so
it asked a plan to pass `/harden`.

The design dialogue that followed found two more faults with one root. The
Changes panel's **Ready** section offered Apply on every change whose thread had
finished, including unhardened ones that Apply would harden first. And a turn
end proposed unhardened work at all. So the user met the hardening at Apply,
instead of the agent meeting it at turn end.

## Decision

- **A branch that lands only plan files never hardens.** Plans never ship to the
  public mirror and have no tests. Hardening runs on the branch that implements
  the plan.
- **Unhardened Lucidos-source work is never proposed.** The proposal hold
  (`read_proposal_hold`) adds `HardeningMissing` beside the plan floor's holds.
  A held turn end emits `ProposalWithheld { reason: hardening_missing }` and
  nudges a live agent to run `/harden`.
- **Ready means *apply-ready***: the thread has finished and the change needs
  no hardening. The engine decides it as `Change::apply_ready`, and Ready, the
  Changes badge, Apply All and Discard All all read it.
- **A resting thread whose work waits on hardening is Blocked**, and its Not
  ready strip offers **Harden** (`POST /api/v1/threads/:id/harden`).

## Rationale

- **One predicate per question.** `git_ops::lands_plan_files_only` and
  `git_ops::needs_hardening` are each defined once. The plan floor, the
  proposal hold, both Apply gates and the change row read them, so no surface
  can disagree with another.
- **A section that offers Apply lists only what Apply merges as it stands.**
  Listing changes Apply would harden first made the button lie about what it
  does.
- **The agent can fix what the engine detects.** At turn end the session is
  still there to run `/harden`. At Apply, the user waits for it.
- **An unknown never skips the gate.** An unanswered landing read counts as work
  that needs hardening, and an empty list is no evidence of a plan.

## Consequences

- *Harden-at-apply* stays, as the backstop for rows proposed before this hold.
  Both Apply gates ask `apply_must_harden`.
- A pending change that needs hardening is legacy only. It sits in **Not
  finished** with " · Not hardened", and the thread's own Apply still hardens
  and merges it.
- `apply_now` stamps `hardened` truthfully: the marker was present, or the run
  just finished. It used to stamp `true` for every branch it merged.
- An app thread owns its hardening, so the change row reads the thread kind
  through the pending-state enrichment.
- A held turn end treats an open change as the unfinished-turn path does. One
  the turn committed past goes back to unproposed work, or Apply would merge
  commits it never covered. One that still carries all the work stands.
- A standing apply drops on work held for hardening, and tells the owner why.
  Waiting there would break ADR 0168's promise that an arm always ends, since
  nothing guarantees the hardening runs. The owner re-arms, or presses Harden.

## Alternatives considered

- **Skip hardening for plan-only branches and change nothing else.** It fixed
  the failure the user hit, and left Ready offering Apply on changes that would
  harden first. The user rejected that inconsistency.
- **Call the state "hardening-ready".** A plan-only change is not hardened, so
  the name would describe the opposite of the case it was made for.
- **Keep Ready as "thread finished" and add a separate apply-ready badge.** Two
  meanings of ready on one row, and a section whose Apply All acts on changes
  it would harden first.
- **A Harden button on the Changes panel row.** No unhardened change reaches the
  panel any more, so the strip on the thread is where the work waits.
