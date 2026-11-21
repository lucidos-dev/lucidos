# 0710: Stop restores unread chat inputs into durable compose in the removal transaction

- **Status**: Accepted
- **Date**: 2026-11-16

## Context

Stop removed unread chat inputs durably but returned their text and images only
in its HTTP response. Losing that response hid the message without restoring
the draft. An engine restart could not recover content from a client response.

## Decision

Stop appends each unread input's text and distinct images to the current durable
compose slot in the same transaction as its deterministic removal event.
The event carries the resulting compose report for projection replay.
The epoch advances, fencing writes composed before restoration.

Reports also carry a bounded journal of exact restored text and images, keyed
by restoration epoch. Compose PUTs retain it; consumption clears it. Reload,
SSE, stale-write refusals and removal replay all deliver the same journal.
A pending client rebases additions newer than its editing baseline exactly
once, recording a new pending edit. Only an acknowledgement covering that edit
can clear it. This distinguishes restoration from a PUT committed before Stop
whose successful acknowledgement arrives afterward; comparing text prefixes
or subtracting image sets cannot distinguish those histories.

## Rationale

The consumption claim and compose write share the thread append lock and
transaction. A delivery race has one winner. Concurrent removals share one
event identity, so only one restoration commits. Compose limits refuse the
removal rather than silently dropping content.

## Consequences

- Broadcast the compose report after commit; reload also recovers the draft.
- Reconcile reports by revision instead of appending the HTTP response again.
- Preserve newer stored drafts and local edits under the compose reconciliation contract.
- Replay applies the saved report only to an older compose epoch.
- A restoration failure leaves the input intact and does not prevent live cancellation.
- Registered unstamped human browsers remain unattested; registration enables take-back without granting owner authority.
- Automation cancellation, coding-agent Stop and trash-button restoration keep their existing contracts.

## Alternatives considered

- **Replay complete HTTP responses.** This adds a generic response store while compose already provides durable state and broadcasts.
- **Client-only restoration.** Lost responses cannot be repaired after reload.
- **Remove despite a full compose slot.** This loses the user's recoverable draft content.
