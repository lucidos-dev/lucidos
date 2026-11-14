# 0582: A prefill beat acts only on the request it sent, and give-up keeps its cause

- **Status**: Accepted
- **Date**: 2026-10-26
- **Amends**: [0580](0580-keep-alive-missed-ping-is-thread-scoped.md), how a
  beat finds its request and what give-up leaves behind.

## Context

ADR 0580 ends a missed prefill beat's run with a tombstone. Two gaps remained.

A beat matched its request by time: a request remembered at or before the
beat's start counted as the one it sent. A request made at the same instant
was taken for the old one. A read on an old request also restarted the
lifetime of whatever request the thread held by then.

Give-up dropped the thread's pair. The status then read `restarted`, though
the engine never restarted.

## Decision

1. **A beat acts only on the pair it sent.** The store shares each pair, and
   a beat's read, miss or talkative end compares the pair itself, never a
   timestamp. A newer request keeps its own lifetime and eligibility.
2. **Give-up leaves a tombstone.** The pair goes and the provider stays. The
   thread reads `past_give_up_time` until it stops waiting. An earlier miss or
   expiry keeps its own cause. A run that pinged still reads `stopped`, and a
   provider-wide pause still outranks the tombstone.

## Rationale

Equal timestamps cannot tell two requests apart, and identity can. A status
that names the wrong cause sends the user looking for a restart that never
happened, which ADR 0580 already ruled out for expiry.

## Consequences

- The store holds each pair behind a shared pointer, so a beat holds no copy.
- Closing spend keeps ADR 0580's rule: persisted calls replace the stored
  count once they hold more beats.
- The tombstone leaves with the thread, as ADR 0580's tombstones do.

## Alternatives considered

- **Keep the timestamp match.** Two requests in one instant still collide.
- **Forget the pair at give-up.** The status claims a restart.
