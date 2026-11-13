# 0527: The ladder's intelligence band ships the blended score in 2-point steps, read in hundredths, superseding 0518

- **Status**: Accepted
- **Date**: 2026-10-19

## Context

ADR 0518 shipped the intelligence band as the blended score in 5-point steps,
and the label read in tenths. Higher efforts of one model sit a point or two
apart, so they shared a band. GPT-6.1 Sol at medium, high and xhigh (48, 50,
51) all read `1×`. Opus 5.5 at high, xhigh and max (53, 55, 57) all read
`1.1×`. The owner reported that a higher effort cannot read as the same
intelligence.

## Decision

The band is the blended score divided by 2, rounded half up. The intelligence
label reads in hundredths: `0.96×`, `1.04×`, `1.16×`. Cost and time labels keep
their tenths. The raw score still never ships.

## Rationale

One step near the reference is now 0.04×, so the label must read in hundredths
to show it. Half-up rounding keeps odd scores two apart in different bands,
where Python's half-to-even `round()` maps 55 and 57 both to 28.

The owner chose 2-point steps over the raw score. A 2-point band still holds a
Lucidos blend, not Artificial Analysis's figure.

## Consequences

Someone who knows the reference rung's score can read the other scores back to
within about 1 point, against 3 before. Two rungs 1 point apart can still share
a band.

## Alternatives considered

- **Ship the raw blended score (1-point steps).** Every distinct score reads
  distinct, but it ships the blend itself. The owner preferred the coarser
  step.
- **Keep 5-point steps and only add decimals.** The bands still collide, so
  decimals show the same `1×`.
- **Force each model's efforts to rise by a step.** Invents a difference the
  estimate does not hold.
