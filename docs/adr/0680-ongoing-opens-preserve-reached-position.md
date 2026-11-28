# 0680: Blocked and Review opens preserve a saved position at or past their target

- **Status**: Accepted
- **Date**: 2026-11-13

## Context

ADR 0409 made Blocked and Review opens land on their target.
That overrode a saved reading position even after the reader had reached the target turn.

## Decision

Restore the saved position when it is at the live edge or names the target turn or a later loaded turn.
Otherwise keep the existing landing. This amends ADR 0409 for both groups.

## Rationale

Returning to a thread should not move the reader backwards within material they already reached.
Compare exchange indices, since failure cards and saved rows can carry step ids while saved line and turn anchors carry turn starter ids.

## Consequences

Saved rows resolve through their owning loaded turn, including head-clamped turns and continuation fragments.
No record, legacy pixel records, and unknown or older saved turns retain the existing target landing.
Notification taps, event links and explicit deep links keep their unconditional landing.
The rule covers every client and workspace; saved positions remain device-local.

## Alternatives considered

- Always land on the target: moves returning readers backwards.
- Always restore: hides new requests the reader has not reached.
- Compare pixel offsets: variable layout and windowing make them incomparable across visits.
