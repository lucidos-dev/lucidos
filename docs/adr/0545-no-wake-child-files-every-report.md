# 0545: A no-wake child files every report, failures and restart cuts included; amends 0451 and 0503.

- **Status**: Accepted
- **Date**: 2026-10-21
- **Amends**: [0451](0451-spawn-declares-the-wake-mode.md), [0503](0503-build-chain.md)

Plan: `docs/plans/2026-10-21-no-wake-child-files-every-report.md`.

## Context

ADR 0451 filed only a clean report from a `no-wake` child. A failure, a cancel,
an `interrupted` turn, an archive, a discard and a turn the engine cut short
all woke the parent. ADR 0503 carried that rule into build chains: "A non-clean
finish still wakes the planner under any wake mode."

A restart then cut a build chain's fourth member mid-turn. Its planner had
handed off and idled long before, and was not running. The `interrupted` report
woke it anyway. The woken turn failed at Claude Code startup, so the planner
showed an error in Blocked as well as the child. The user had pressed nothing
on the planner, and the planner could do nothing the child's own Continue
button does not.

## Decision

A `no-wake` child never wakes its parent. Every report it sends is a filed
report (`ChildReportFiled`), whatever its status. That covers the cards a
settle and the end of an event wait owe, and a turn the engine cut short.

The boot sweep re-fires no `ChildThreadCompleted` a `no-wake` child left under
the old rule.

`wake`, `wake-when-all-done` and undeclared children keep ADR 0451's rule.

## Rationale

**`no-wake` already means "the user reads the result in the child".** That is
ADR 0451's own definition. A failed or interrupted child shows its own state,
in Blocked, with Continue or Try again. Waking the parent adds a second turn
that can only repeat what that button does.

**The parent declared it at the spawn.** It said it does not act on this child.
A failure does not change what the parent waits for, because it waits for
nothing.

**A woken parent can fail too.** Each wake is a full session start. When it
fails, a second thread lands in Blocked for an error the user can only clear by
running a turn they never wanted.

**Active orchestrators keep their wake.** A parent that spawned `wake` or
`wake-when-all-done` needs to know a child failed or was cut, so it can follow
up. Nothing changes for it.

## Consequences

- A build chain's planner stays quiet whatever its members do. The user reads
  a member's failure or interruption in the member.
- The home thread no longer wakes when one of its `no-wake` children fails.
  The child shows the failure, and Home reads the filed report on its next
  turn.
- A parent that needs to hear of a failure spawns with `wake`, as it already
  must to hear of a result.
- A filed report never writes the parent's row, so a parent on a question, an
  event wait or a stopped child keeps that state.

## Alternatives considered

- **File only an `interrupted` report from a `no-wake` child.** Narrower: it
  fixes the restart case and keeps a real failure waking. Rejected by the
  user. A `no-wake` child handles its own result, error or success, and the
  child already shows a failure.
- **Never wake a handed-off parent.** It would cover a `wake` child of a thread
  that later handed off too. Rejected for now: only a build chain leaves a
  handed-off thread as the parent of working children, and its members are
  `no-wake`. A `wake` child's report filed on a thread that never runs again
  would be lost.
- **Gate on whether the parent was running before the restart.** An idle
  orchestrator waiting for a `wake` child is idle too, and must still wake.
  Being idle says nothing about whether a report is owed.
