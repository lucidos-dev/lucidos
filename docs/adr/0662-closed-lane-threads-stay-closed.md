# 0662: A closed landing lane thread cannot resume as an ordinary merge worker

- **Status**: Accepted
- **Date**: 2026-11-11
- **Amended by**: [0675: A lane thread's handoff starts a fresh session, and a closed lane thread neither hands off nor replays one](0675-lane-thread-handoffs.md)
- **Amends**: [0463](0463-landing-lane.md)

## Context

A failed batch released its source changes. Continue then started its former
lane conversation on an ordinary branch, still carrying the instructions to
merge those sources. Requeued sources opened fresh batches alongside it.

Another resolver received a text instruction to stop and ended cleanly before
hardening. The engine verified its merge structure and landed it anyway.

## Decision

A lane thread retains its batch identity for its lifetime. After the batch
closes, admission refuses every later run with an actionable explanation.
Pending source changes can be applied again through the lane.

Stale cleanup and the startup held-back sweep cannot propose historical ordinary work from a closed lane thread.
Cleanup rescues uncommitted edits and records the historical branch as withheld.
The idle reason tells the user to recover it in a new coding-agent thread.
Explicit Discard still cleans up that historical ordinary work.

Finalization requires current full hardening for the integration head, including
a head changed by catch-up. Missing certification keeps the batch open and
non-finalizing. Engine Stop preserves the open batch for Continue.

## Rationale

Removing an integration branch does not remove the conversation's merge
instructions. Reusing that conversation as an ordinary thread creates another
worker for released members. Persisted batch identity fences it across recovery.

Clean merge structure proves neither review nor passing suites. The final
publication boundary must enforce the hardening contract the prompt describes.

## Consequences

Closed lane conversations cannot be repurposed as ordinary coding-agent threads.
Stopped open batches remain resumable. Failed batches still release pending
members. A clean catch-up that changes the head requires hardening again.
Text instructions do not replace the engine's Stop operation.

## Alternatives considered

- Retain failed members forever: prevents retries after genuine failures and
  preserves ownership for a thread that may never return.
- Remove the old branch before ordinary admission: leaves stale merge
  instructions intact and permits independent duplicate work.
- Trust a clean turn as proof of hardening: a resolver can stop its work without
  leaving unresolved paths, yet never run the required checks.
