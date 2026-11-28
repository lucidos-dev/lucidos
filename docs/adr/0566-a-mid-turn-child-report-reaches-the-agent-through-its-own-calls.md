# 0566: A child report reaches a mid-turn coding agent through its own lucidos calls, never an interrupt

- **Status**: Accepted
- **Date**: 2026-10-23

## Context

A coding-agent parent could not see a child's report while its own turn ran.
The fan-in forwards the report as an input, and an agent reads input only at a
boundary: Codex at the next turn, Claude Code at its next tool boundary. A
Codex parent spawned a review child, then began a long `/harden` turn. The
child finished, the user saw its card, and the parent worked blind for the
rest of the turn. It started a second review of the same diff.

Plan: `docs/plans/2026-10-23-child-report-reaches-a-mid-turn-agent.md`.

## Decision

The agent pulls the report through its own `lucidos` calls. Most calls note a
waiting report on stderr, `lucidos background-task wait` returns early with
it, and `lucidos threads child-reports` reads it. Taking a report withdraws
its queued copy from the driver and records `ChildReportTaken`, so the agent
reads each report once. The engine never interrupts a turn for one.

## Rationale

- **An interrupt throws work away.** A child report is news, not a
  cancellation. Stopping a turn mid-build to deliver it costs the build, and
  `should_redirect_followup` already refuses to redirect a turn for an engine
  re-entry.
- **The agent already calls `lucidos` often.** A long turn polls a
  background task, and each poll is a natural point to hand over news. The
  call is the agent's own tool boundary, so the report lands where the agent
  can act on it.
- **Exactly once needs a withdraw, not a flag.** The queued copy is in the
  driver, or in Claude Code's own input queue. Only taking it back there
  guarantees the turn-end delivery never repeats what the agent read. A
  report the driver already handed over answers `AlreadyRead` and is not
  taken, so nothing reads twice in either direction.

## Consequences

- Both Codex drivers now answer a withdraw: an input still in the driver's
  queue is dropped. One Codex already took stays unwithdrawable.
- `ChildReportTaken` follows the card on the parent, so the boot refire
  treats the card as handled.
- A coalesced write is not taken: the ledger cannot withdraw part of it, so
  that report still arrives at turn end.
- An agent that makes no `lucidos` call during its turn still reads the report
  only at turn end. Nothing pushes it.

## Alternatives considered

- **Interrupt the turn.** Delivers at once, but loses the in-flight work and
  breaks the rule that only a user follow-up redirects a turn.
- **Only the early `wait` return.** Smaller, but an agent holding no
  background task never learns of the report. Rejected by the user.
- **Copy the report into the call's answer and leave the queued copy.** The
  agent then reads the same report twice, once mid-turn and once at turn end,
  and may act on it twice.
