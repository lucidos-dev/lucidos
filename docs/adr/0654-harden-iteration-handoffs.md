# 0654: Hardening evaluates a successor checkpoint between review iterations and preserves incremental review and validated suite evidence

- **Status**: Accepted
- **Date**: 2026-11-11

## Context

The first-entry `pre-harden` checkpoint cannot bound a long review loop.
Every retry returns to Phase 1 without consulting the engine again. Restarting
the playbook deletes the review head and loses incremental scope (ADR 0476).

## Decision

Run the flow-owned `harden-iteration` checkpoint once after a failed
iteration records its verdict and commits fixes. Use the configured ceiling
and handoff estimate through the existing successor CLI (ADRs 0505 and 0629).
A successor resumes scope selection directly, skipping the checkpoint once,
preserving the tier, review head and verification evidence.

## Rationale

An iteration boundary is safe to settle session-owned work and write an
accurate continuation. A final passing iteration needs only its marker, so
handing off there buys nothing. Both coding agents and both hardening tiers
share the same contract; no workspace-specific setting defines the policy.

## Consequences

The narrative carries review scope, findings, commits, iteration number,
tier, process status, suite evidence and the next command. The existing suite
validator remains authoritative: missing exits, invalid inputs and passing
retry subsets cannot certify a full run. Missing review state widens review.
Session-owned processes must be joined or stopped before handoff.
Existing external flow copies adopt this policy when updated.

## Alternatives considered

- A fixed 200k threshold duplicates engine policy and ignores configured
  ceilings and cost estimates.
- Restarting Phase 0 deletes incremental review state.
- Mid-tool handoff can interrupt verification and lose process results.
- Trusting a narrative PASS bypasses suite input and exit validation.
