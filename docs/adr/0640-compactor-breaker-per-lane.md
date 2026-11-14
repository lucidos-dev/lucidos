# 0640: The compactor's circuit breaker lives per lane, not in the model call service

- **Status**: Accepted
- **Date**: 2026-11-09

## Context

When the compactor's model was down, the compactor retried every failed node every
10 seconds. Each retry was a billed call. One outage logged about 94,000 failed
calls, while memory extraction logged 283 in the same window. Nothing told the
user the model was down.

## Decision

The breaker lives in the compactor (`summary_tree/breaker.rs`), keyed by
`(lane, model)`. The model call service stays unchanged.

## Rationale

The compactor is the only self-refeeding background loop. Its queue refills itself,
so its failures grow with time, not with user activity. Every other background
purpose makes one call per user or agent action, with at most 3 tries.

A breaker in the model call service would also gate interactive turns. The user of
an interactive turn sees the error and decides what to do. A hidden gate would
turn their error into a silent wait. It would also need a policy per purpose:
which failures count, how long to wait, and who to tell.

The compactor knows its own policy. It knows which failures are node-local, how
the backfill reports progress, and where the user picks another model.

## Consequences

- Three failed calls in a row open the breaker. One probe call goes after each
  wait. The wait starts at 10 seconds, doubles, and caps at 5 minutes.
- The Memory row names the failing model and links to the model picker. One
  notification follows when the wait first reaches its cap.
- Picking another compactor model gets a fresh breaker at once.
- A Flex capacity refusal counts nothing. It has its own fallback to the standard
  tier, and counting it tripped the breaker on a healthy provider. The probe
  therefore asks for the standard tier: a neutral refusal would hand the probe
  on at once, with no back-off.
- Only the probe's own failure lengthens the wait. A late failure from a call
  admitted before the trip says nothing new.
- Another background purpose that grows a self-refeeding loop needs its own
  breaker. This decision does not cover it.

## Alternatives considered

- **A breaker in the model call service.** Rejected for the reasons above: it gates
  interactive turns and needs a policy per purpose.
- **A longer fixed retry interval.** Rejected. It slows recovery after a short blip
  and still bills a call per node per interval during a long outage.
- **Pausing the whole compactor on the first failure.** Rejected. One bad request
  among successes would stop the backfill. Any success resets the streak instead.

## Amendment, 2026-11-10: unattended writes and spend cap holds

A typed spend-cap refusal does not count toward the breaker or halve its lane.
A provider hold gates background calls until reset, while interactive turns still send.
See [ADR 0652](0652-unattended-writes-and-spend-cap-holds.md).
