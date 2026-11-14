# 0518: The ladder's intelligence band ships a blended score in 5-point steps, never the raw score

- **Status**: Superseded by
  [ADR 0527](0527-intelligence-band-in-2-point-steps.md)
- **Date**: 2026-10-18

## Context

The ladder (ADR 0500) shows cost and time as multiples of the owner's default
rung. The owner asked for a third band, intelligence, read the same way. Order
and the frontier flag already come from the estimated score, but the score
itself never shipped, because Artificial Analysis (AA) forbids republishing its
figures.

## Decision

Each rung in the menu files carries `intelligence_band`: the estimate script's
blended score (75% AA, 25% Epoch ECI) divided by 5 and rounded. The engine
divides a rung's band by the reference rung's band at serve time. So the
multiple follows the owner's default as cost and time do. No raw score ships.

## Rationale

A multiple needs a number to divide. A 5-point step keeps the number coarse
enough that it is a Lucidos band, not AA's figure. It still orders the rungs
and shows how far apart they sit.

## Consequences

Someone who knows the default rung's score can read the other scores back to
within about 3 points. The figure is a Lucidos blend of two sources, not AA's
own number, and the owner accepted that trade-off. A rung's band moves in whole
steps, so two rungs a few points apart can read `1×` together.

## Alternatives considered

- **Ship the raw blended score.** Exact multiples, but it republishes a figure
  derived from AA's. Rejected.
- **Show a rank instead of a multiple.** Safe, but it drops how far apart the
  rungs sit, which is the point of a band.
- **A wider step (10 points).** Safer, but with scores in a narrow range most
  rungs would share a band.
