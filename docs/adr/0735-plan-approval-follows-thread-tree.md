# 0735: Plan approval follows a version within a repository and thread tree

- **Status**: Accepted
- **Date**: 2026-11-18

## Context

A parent approved a plan, then its child and the parent each asked again.
The branch marker was local to the branch and Apply consumed it.
This decision amends [ADR 0397](0397-turn-end-holds-a-change-the-plan-floor-would-refuse.md).

## Decision

An implementation approval is reusable for the same plan path and version in
the same repository and thread tree.
A version tolerates the status line change made by `planned approve`.
An inherited approval names the engine-checked owner approval that granted it.
Plan-only approvals grant no implementation authority.

## Consequences

Apply still consumes the branch marker.
Re-marking the same version can recreate it from the approval event.
An edited plan body needs a fresh approval.
A new child can carry the parent's approved marker, even if its base predates the plan file.
If its branch contains that path, its committed version must match.
Historical approvals without a repository and version binding remain readable
and cannot grant inherited approval.
