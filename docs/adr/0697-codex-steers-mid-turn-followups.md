# 0697: Codex steers ordinary follow-ups during a live turn

- **Status**: Accepted
- **Date**: 2026-11-15
- **Amends**: ADR 0005, mid-turn follow-up addendum

## Context

Interrupting every Codex follow-up ends accepted work early. The app-server
now serves `turn/steer`, which accepts input without ending the active turn.
CLI version strings cannot prove that a running server supports the method.

## Decision

Probe `turn/steer` during handshake against a fresh nonexistent thread with
empty input. A handler-specific `thread not found` refusal proves support.
Unknown methods, inconclusive replies and timeouts retain legacy routing.
The session Init reports the negotiated capability. Exec cannot steer.

Ordinary follow-ups steer capable sessions using the current `expectedTurnId`.
Urgent follow-ups interrupt on both backends. A successful steer emits one
read receipt and contributes to the same turn's single result.

A turn-id race keeps the input queued for the next turn. Other refusals disable steering for
this process and use interrupt-and-redirect without discarding the input.
Only a successful acceptance marks an input read. A terminal arriving before
a steer acknowledgement waits for that acknowledgement before publication.

An acknowledgement timeout fails visibly rather than silently losing input.

Codex result text selects marked final answers, falling back to all completed
messages when none are marked. Streaming commentary remains visible.

## Rationale

Steering lets a follow-up refine accepted work without ending it. Capability
comes from the process that will receive input, and a nonexistent-thread probe
cannot alter a user turn. Queued rejections preserve the input ledger.

## Consequences

Every workspace and both shipped platforms use the server's actual capability.
Older app-servers and exec retain interrupt-and-redirect. Models without message
phases retain their existing joined result. No general outstanding-work detector
is introduced. Upgrades and downgrades are re-evaluated per session.

## Alternatives considered

- Always interrupt: ends the work a plain follow-up was meant to steer.
- Infer support from a CLI version: does not establish server capability.
- Probe a live thread: could alter user work during capability detection.
