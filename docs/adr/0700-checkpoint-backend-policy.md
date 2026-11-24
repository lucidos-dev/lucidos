# 0700: Checkpoint backend choices follow repository flow policy rather than installation

- **Status**: Accepted
- **Date**: 2026-11-15

## Context

ADR 0691 reserved an other-backend approval option merely because its ladder
was read. This made installation dictate a workflow choice, even when the user
wanted straightforward implementation on their preferred backend.

## Decision

Amend ADR 0691's mandatory X option. Repository flow instructions configure
backend policy at offered checkpoints. Without a policy, keep the current
backend. A user request or flow policy may permit comparison or prefer another
backend, but intelligence fit takes priority over preference. Readable ladders
establish capability, not a requirement to offer it. X is optional and merges
into R when both name the same backend.

This repository prefers Codex for straightforward implementation with settled
decisions. Its policy lives outside the reusable default-flow block. Keep
plan-only approval, slot limits and automatic-checkpoint authorization unchanged.

## Rationale

Repositories already own their checkpoint instructions. Using that surface
keeps policy close to the work and avoids adding another configuration layer.
A backend preference should not force insufficient intelligence or crowd out
plan decisions with irrelevant alternatives.

## Consequences

Codex and Claude Code sessions follow the same policy contract. Cross-backend
handoffs remain supported with the ladder's full implementer flags. Generic
flows stay neutral; repository policy and explicit user choices supply intent.
Generated CLI examples distinguish defaults from permitted alternatives.

## Alternatives considered

- Always reserve the other backend: confuses availability with intent.
- Forbid cross-backend recommendations: prevents appropriate implementer choices.
- Add a CLI flag or settings store: duplicates the existing flow configuration.
- Hardcode Codex in generic CLI guidance: imposes one repository's preference
  on every user and repository.
