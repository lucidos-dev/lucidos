# 0802: Paired devices can confirm several named registered workspaces in one delete batch

- **Status**: Accepted
- **Date**: 2026-11-23
- **Amends**: [ADR 0620](0620-workspace-cleanup-authority.md), [ADR 0664](0664-permissions-and-the-pause-family.md), [ADR 0671](0671-workspace-broker.md)

## Context

Deleting a registered workspace from the picker required a machine-wide cleanup review.
A paired device already knows which registered workspace rows the person selected.

## Decision

A paired device freezes the selected registered workspace IDs through the workspace delete batch route.
The picker names every frozen member in one danger dialog, including workspaces with a protected origin.
The gateway identifies members whose database will be dropped without a backup. The dialog warns before deleting them.
After confirmation, it echoes the frozen phrase and the gateway revalidates each member before deletion.

An agent proposes `workspace_deletion` as a batch item with the exact workspace slugs.
The batch card names every slug and a deletion-only batch offers **Allow once**.
After confirmation the engine spends the grant and calls the workspace broker once per slug.
The broker checks the current origin under the workspace lock and refuses protected workspaces and the calling workspace.
The batch receipt records any missing or refused target and any deletion whose outcome is uncertain.

## Rationale

The person's consent covers the named set.
Observing only selected folders and databases keeps deletion independent of the review cache and unrelated resources.

## Consequences

The cleanup review retains its individual confirmation for protected resources.
Deletion still uses the existing ownership checks, Trash journals and audit records.
Agent authority is limited to machine-made workspaces and the named set requires one-time permission.

## Alternatives considered

- Keep a typed phrase for picker deletion. A dialog naming every member supplies the required confirmation.
- Refresh the cleanup review first. That adds a machine-wide scan to a request for a known set.
