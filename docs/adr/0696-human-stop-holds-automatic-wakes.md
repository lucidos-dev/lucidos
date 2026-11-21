# 0696: A human Stop holds automatic thread wakes until a human resumes

- **Status**: Accepted
- **Date**: 2026-11-15
- **Amends**: ADRs 0252 and 0664

## Context

Stop ended one turn, but a later event-wait delivery, child report or engine
nudge could immediately start another. An automation pause covered only
coding-agent threads and raised permission cards instead of holding silently.
A human Stop must hold chat and coding-agent threads across engine restarts.

## Decision

Fold the stopped state from existing thread events. A device-attributed
`ResponseCanceled { cause: user_stop }` starts the hold. A device message,
Continue, card answer, Harden or Apply ends it. Agent and engine events do
not clear it. An unreadable fold holds automatic work.

Automatic steps return `StepVerdict::Held` before the automation-pause check.
No permission request is raised. Chat engine re-entry, queued coding-agent
input, keep-alive selection and boot recovery also check the same fold.
Reports are filed and waits retain their unread anchors. A human resume
carries the held wait text after its own words; child reports remain in chat
history and coding agents' turn-gap notes. Turn-end nudges are recomputed,
not replayed.

An agent message from Home or a parent may still start a turn while stopped.
It does not lift the hold on subsequent engine wakes. Home's wider reach is
only for automation pause, not Stop, Apply or other acts.

## Consequences

This applies to every workspace and both thread kinds, on macOS and Linux,
with no new event or in-memory state. Existing subscriptions survive Stop
(ADR 0052), and the Stop's own idle still emits `CodingAgentIdled` (ADR 0252).
A stopped child's quiet parent note and callback duty are unchanged.

## Alternatives considered

- Reuse automation pause: rejected because it raises cards and covers only
  coding-agent threads.
- Store a live-session flag: rejected because restart would clear the hold.
- Block agent follow-ups too: rejected in the approved implementation plan.
