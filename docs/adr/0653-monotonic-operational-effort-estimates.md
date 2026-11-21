# 0653: Operational capability estimates never decrease with reasoning effort

- **Status**: Accepted
- **Date**: 2026-11-10

## Context

ADR 0525 allowed a higher effort to rank lower when the Coding Agent Index
scored it lower. The owner reported Sol medium ranking above high and max
and approved replacing that behavior with guarded operational estimates.

The historical input snapshot already contains the inversion. It lacks exact
source variant and harness evidence, so we cannot distinguish benchmark
variation from an import mapping error. Directly converting it into ladder
order treated that uncertainty as a capability regression.

## Decision

Amend ADR 0525's coding-profile clause. Before sorting or frontier selection,
apply a cumulative maximum over effort within each model, on both profiles.
Higher effort cannot receive a lower operational capability estimate. Equal
estimates are valid; raw input observations remain unchanged.

Emit a review diagnostic for each adjustment. Describe smoothing in generated
provenance and require exact variant/harness evidence for future imports.
Reject duplicate pairs, unsupported efforts and invalid coding scores.
Independent shipped-data tests and generator regressions guard the policy.

## Rationale

A benchmark fluctuation is insufficient evidence to demote a higher effort
in an operational ladder. Smoothing states an explicit estimation policy,
without pretending the observations prove strict improvement.

## Consequences

All shipped ladder surfaces use consistent effort estimates. Workspace
customizations remain authoritative. Covered coding scores may be smoothed;
`coding_estimated` still means the index did not cover that pair.
Cost and time estimates remain unchanged. A higher effort can remain off
when it offers no estimated capability advantage for its cost.

## Alternatives considered

- Preserve benchmark dips: rejected because they mislead operational effort selection.
- Force a strict gain per tier: rejected because it invents differences, as ADR 0527 explains.
- Change only UI sorting: rejected because labels and operational rung selection would still disagree.
