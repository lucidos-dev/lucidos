# 0762: A release has one notice, and the modal answers the items it shows

- **Status**: Accepted
- **Date**: 2026-11-22

## Context

A single release can ask a workspace to review several changes. The
announcement modal listed individual notice titles and sent the reader to
Settings to read and answer them. Starting an action there could leave the
rest of the backlog waiting for another visit.

## Decision

Group visible notice items by their authored release. The modal shows owed
releases oldest first and can answer all the items it shows. Settings uses the
same cards and keeps answered releases available under **Already answered**.

**Got it** and a successful **Check my workspace** start answer the displayed
owed items. Closing the modal or following a link leaves them owed. The engine
builds one prompt from the displayed items' checks and action sentences.

## Rationale

Short summaries and expandable details make the modal a place to read and
answer the backlog. One check thread can cover all the checks the reader saw.
The engine compares the ordered item ids with the current owed sequence before
advancing the cursor. A condition can change while the modal is open, even
when the last owed id stays the same.

## Consequences

The cursor still holds an item id in the full authored sequence. A stale
answer refreshes the view, and concurrent answers can only advance the
cursor. Older clients can still answer the first owed item individually.

[ADR 0130](0130-release-notice-backlog-on-entering-the-sequence.md) still
governs how a workspace enters the sequence.
[ADR 0214](0214-one-shot-migrations-are-a-notice-plus-an-audit-check.md)
still assigns detection and remediation to the workspace audit.

## Alternatives considered

The [announcement-only modal](../plans/2026-10-21-release-notice-modal-points-at-the-panel.md)
kept the backlog visible in Settings, but required a separate visit to read it.
A release-level cursor would discard the item position needed for conditional
visibility and workspaces that already answered part of a release.
