# 0497: Under the context threshold, the plan card offers a new thread only when the agent's recommended implementer is cheaper than the planning thread's; amends 0486.

- **Status**: Accepted
- **Date**: 2026-10-16

## Context

The plan-approval card offered `Implement in a new thread` only over
`HANDOFF_OFFER_TOKENS`. ADR 0486 kept it that way: "Whether to offer a new
thread at all stays the measured context's call."

A planning thread on Opus wrote a fully specified plan under the threshold. The
card had no new-thread option, so the build ran on Opus. The user asked for the
option there too, since such a plan suits Sonnet.

## Decision

Two reasons offer a new thread. Over the threshold, the context offers it, as
before. Under it, the card offers it only when the planning agent's
recommended implementer is cheaper than this thread's, by model or effort.
The small card then reads
`Implement in a new thread: <implementer> (Recommended)`, `Approve here`, and a
real fork if there is one. Otherwise the card is the plain floor.

The engine's context reading gains `can_hand_off`, so a thread that cannot
start a successor never gets the option at any size.

## Rationale

A new thread saves money in two ways. A fresh context makes every call cheaper,
which pays only on a large thread. A cheaper implementer makes every call
cheaper at any size. The first is a number the engine measures. The second is
the agent's reading of the plan, which ADR 0486 already trusts for the pick
itself.

A new thread on this thread's own implementer saves nothing on a small context.
So the small card never offers it: that would be `Approve here` with extra
steps.

## Consequences

- The offer under the threshold now rests on the agent's judgement. A poor pick
  costs one tap, as ADR 0486 accepts for the pick itself.
- `offer` keeps its meaning. The composer's successor strip still appears only
  over the threshold.
- The small card's text lives beside the large one in `card_shape`
  (`crates/lucidos-cli/src/planned.rs`).

## Alternatives considered

- **Always offer a new thread.** On a small context with the same implementer
  it saves nothing and adds a handoff.
- **Lower the threshold.** A smaller number still misses a small, fully
  specified plan, which is the case that prompted this.
- **An engine-side check that the pick is cheaper.** The pick is prose on a card,
  so the engine never sees it before the user does.
