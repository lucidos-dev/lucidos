# 0708: Plan approval always offers a fresh session and every readable backend

- **Status**: Accepted
- **Date**: 2026-11-15

## Context

A plan approval omitted fresh-session choices even though the estimate recommended handoff.
ADR 0700 allowed policy to hide the alternate backend. Truncated CLI output
also hid the verdict above the ladders.

## Decision

Amend ADRs 0497 and 0700 at plan approval: always include R when handoff is available.
Include X when another backend ladder is readable, merging it into R when
R names that backend. A non-paying R omits `(Recommended)`, which moves to
`Approve here`. The recommended option always displays first.

Repeat the verdict at the end of every offered checkpoint output.
The default flow requires reading the whole output without truncation.

## Rationale

The estimate and repository policy guide recommendations. They must not remove
the user’s ability to choose a fresh session or another readable backend.

## Consequences

Keep the four-option limit and existing slot priority. A fork without a slot
remains a typed reply. Unavailable readings offer no handoff. Other checkpoints
retain ADR 0700’s optional backend choices and existing recommendation rules.

## Alternatives considered

- Keep policy-based eligibility at plan approval: hides choices users need.
- Change the estimate: it already recommended handoff in the reported case.
- Require every backend at every checkpoint: exceeds the plan-approval decision.
