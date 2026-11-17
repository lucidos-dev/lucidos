# 0711: Allowing an unattested start adopts the thread

- **Status**: Accepted
- **Date**: 2026-11-16
- **Amends**: [0647](0647-only-a-signed-claim-proves-the-owner.md), [0688](0688-every-allow-once-card-is-a-permission-request.md)

## Context

An unattested caller can ask to start a coding-agent thread.
The owner's Allow once started one run, but every handoff or later input asked again.
The owner means to allow the thread to start, including its later runs.

A refusal also emitted a cancellation for every parked input.
One answer should settle the parked turn with one response.

## Decision

A spent owner-granted start permission adopts the thread.
The provenance walk treats that thread as an owner root on later runs.
Children and other provenance hops inherit that root.
Before checking adoption, each hop collects its feeding triggers and saved-draft submitters.
Those triggers still need their own coding-agent grants.

The first allowed run still spends a once permission with a unique breadth, held by the workspace.
An unspent grant does not adopt anything.
A new thread from the same caller still asks.
Snapshot restore drops adoption when it rewinds past the start card.
Read errors refuse as untraceable.

The start card says **Allow**, described as:
“Allow it for this thread.”
Its question explains the same thread scope.
HTTP acts keep **Allow once** and run once.
Only an owner's device tap grants either card.

Don't allow emits one cancellation anchored to the latest parked start.
Recovery excludes every input through that anchor, including older per-message refusal records.
Later input remains eligible for recovery.

## Consequences

Later input, continuations and successor sessions in an adopted thread start without another unattested-start card.
Adoption changes the provenance root, not the caller's signed identity or tool permissions.
No new storage or in-memory authority is needed: spent permissions and their start cards hold the evidence.
No revocation UI is added; archiving ends the thread's runs.

This applies to every workspace on macOS app and macOS/Linux headless installs.
The engine reads workspace-local evidence and assumes no gateway or network layout.

## Alternatives considered

- **Keep asking per run.** Rejected: a handoff repeats a decision the owner already made about the thread.
- **Trust the caller across threads.** Rejected: allowing one thread does not authorize another.
- **Make every unattested act persistent.** Rejected: HTTP requests remain exact, single-use acts.
- **Adopt before spending.** Rejected: only the resumed run may consume the owner's start grant.

## Verification

Spawn-gate tests cover spending, later runs, inherited adoption, separate threads, feeding triggers and snapshot restore.
Card tests cover wording and the unchanged once permission.
Refusal tests cover one response for multiple inputs, repeat-answer idempotence and recovery boundaries.
