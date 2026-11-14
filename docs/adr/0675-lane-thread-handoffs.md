# 0675: A lane thread's handoff starts a fresh session, and a closed lane thread neither hands off nor replays one

- **Status**: Accepted
- **Date**: 2026-11-13
- **Amends**: [0560](0560-successor-session-in-the-same-thread.md), [0662](0662-closed-lane-threads-stay-closed.md)

## Context

A lane thread handed off at its `pre-harden` checkpoint while its batch was
open. Its next run carried a lane duty, and a lane-duty run never read the
pending handoff. So it resumed the old session and recorded continuity
`resumed`, which does not end a handoff.

The batch landed with the handoff still pending. At the next engine restart
the boot sweep replayed the handoff. Admission refused it on the closed lane
thread (ADR 0662), and the thread showed a red failure two hours after its
work had landed. Two earlier lane threads show the same replay at each restart.

## Decision

1. **A lane thread's own sessions take part in handoffs.** A lane-duty run
   reads the pending handoff under the handoff lock, as an ordinary run does.
   When one is pending, it starts a fresh session from the handoff's opening
   and records continuity `handoff`, which ends the handoff. Without one, it
   keeps the caller's resume id.
2. **Conflict-resolution and orphan-recovery sessions still ignore the
   handoff.** They are not the thread's next session.
3. **The engine never starts an unprompted successor session on a closed lane
   thread.** `successor_to_start` skips an engine handoff once the lane batch
   landed or failed. It emits nothing. A handoff that carries the user's own
   request still starts, and admission answers the user with the refusal.
4. **A closed lane thread cannot hand off.** `handoff_block` returns
   `LaneEnded`, and the API refuses with reason `lane-ended`.

## Rationale

A handoff promises a fresh session. A lane run that resumed the old session
broke that promise and left the handoff live, to be replayed at every boot.
Making the lane run consume it fixes the cause on both backends: the resume
decision runs before the Claude Code or Codex dispatch.

The closed-lane gate is the second half. A handoff recorded before this
change, or one a crash left pending, must not outlive its batch. Skipping it
silently is right, because nobody is waiting on it. A user's own request is
owed an answer, so it still runs and gets the actionable ADR 0662 refusal.

## Consequences

- No stale handoff produces a red failure on a closed lane thread.
- A lane batch that is still open at upgrade time, whose thread carries a
  lingering pre-fix handoff, runs one fresh successor session from it. That
  ends the handoff, so it happens at most once.
- `ends_handoff_sql` keeps its definition.

## Alternatives considered

- **Let any resumed session end a handoff.** Rejected: Tier-2 merge retries
  resume on purpose, and the thread's own successor session must stay pending
  through them.
- **Gate only at admission.** Rejected: admission already refuses, and that
  refusal is the red failure this fixes. The gate belongs before the turn is
  created.
- **Rewrite or close the incident thread's handoff by an event.** Rejected:
  its failed replay already ended the handoff, and history stays immutable.
