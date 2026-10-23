# 0537: The cost band counts sixth-root-of-2 steps; time stays on the cube root

- **Status**: Accepted
- **Date**: 2026-10-20
- **Amends**: 0500

## Context

ADR 0500 put every ladder band on one grid: steps of the cube root of 2, about
26% each. So a cost of "1×" meant anywhere from about 0.89× to 1.12× of the
reference rung. GPT-6 Astra at `medium` costs about 1.15× Opus 5.5 at `medium`
per task. The chat ladder still showed it at 1× on intelligence, cost and time,
which read as a bug to the owner.

## Decision

The token band in `rung_estimates.json` and the served cost band count 6 steps
per doubling, the sixth root of 2. The time band keeps 3 steps per doubling.
The intelligence bands keep their 2-point steps.

## Rationale

- Cost is the column a person picks a rung on, and price differences of 10% to
  20% between neighbours are common. The coarse grid hid them.
- A sixth-root step is still a band: an integer step count, never Artificial
  Analysis's figure (ADR 0525 § bands only).
- Time comes from a model of tokens over speed, calibrated against AA's times.
  It is a rougher estimate than cost, so a finer grid there would claim
  precision the estimate lacks.

## Consequences

- The engine has two grid constants, `COST_STEPS_PER_DOUBLING` and
  `TIME_STEPS_PER_DOUBLING`. The estimate script keeps pinned copies of both.
- Labels move: Opus 5.5 at `max` reads 4.5× where it read 4×.
- The frontier of on rungs uses the finer cost. Two rungs the coarse grid kept
  on by rounding are now beaten and turn off: Sonnet 5.5 at `xhigh` on Claude
  Code (Opus 5.5 at `high` is smarter and cheaper), and Haiku 5.5 at `xhigh` on
  chat.

## Alternatives considered

- **Finer cost and time.** One precision across the columns, but it overstates
  how well the time estimate is known. Rejected.
- **Keep the cube-root grid.** "1×" keeps meaning ±13%, and near-ties keep
  reading as identical rungs. Rejected by the owner.
- **Drop the snap and show the exact multiple.** That is close to republishing
  the estimate inputs, which ADR 0525 rules out. Rejected.
