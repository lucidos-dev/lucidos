# 0557: Harden hands off onto the default rung, or one rung above it for security work or a costly one-way door

- **Status**: Accepted
- **Date**: 2026-10-23
- **Amends**: [0499](0499-successor-offer-follows-effort-and-checkpoints.md), [0505](0505-named-handoff-checkpoints.md)

## Context

Under ADR 0505 an automatic checkpoint hands off only when the *handoff
estimate* pays, and always on the thread's own implementer. A builder on a
cheap rung, such as Sonnet 5.5 medium, therefore also hardens on it. The
`/code-review` subagents inherit the thread's model, so the review shares the
builder's model and its blind spots.

Plan: `docs/plans/2026-10-23-harden-floor-rung.md`.

## Decision

- **A flow may declare the *default floor* for one run.**
  `lucidos successor offer --at <name> --default-floor` makes the surface's
  default rung the floor. The shipped hardening flow block passes it. The
  engine still names no checkpoint (ADR 0505).
- **The engine decides, from the ladder.** It names a *floor rung* (the
  default) to a thread on the ladder below it. It names a *risk rung* (just
  above the default, never the top) to a thread below that. It names neither
  when the ladder does not climb, the thread runs off it, cannot hand off, or
  holds an open change.
- **A floor rung hands off even when the run does not pay.** Pays and raises:
  hand off on the floor rung. Pays only: hand off on this thread's
  implementer, as before.
- **The agent takes the risk rung on either of two tests**, and says why in
  one line:
  - *security work*: a security or authority boundary, secrets, credentials,
    pairing, sandboxing, or what an agent may do;
  - *a costly one-way door*: a migration or new stored format, a published
    contract, a release or install format, or destroyed data or git work.

  Everything else stays on the floor.
- **Either rung is a standing consent**, on the same terms as a paying run
  (ADRs 0503, 0507). It covers a spawn at that checkpoint on exactly the rung
  the latest run named, nothing else.
- **The CLI keeps no copy of the ladder** (ADR 0500). It prints the model and
  effort the engine returned.

## Rationale

- **The review is where intelligence pays.** A missed defect lands on `main`.
  The fixes are small, and the suites need little reasoning.
- **A fresh model reads the diff without the reasoning that produced it.**
  Sonnet 5.5 high is 0.96× on paper, but it is still the builder's model.
- **The default rung, not a named model.** The owner sets it, and the ladder
  holds it. A named model would be a second definition of "good enough" that
  drifts at the next release (`.claude/rules/one-definition-per-value.md`).
- **A floor, never a cap.** A thread at or above the default keeps its rung.
- **The suites stay cheap anyway.** The successor starts small, so a polling
  call on Opus at 20k tokens costs less than one on Sonnet at 250k.
- **One rung up costs at most 1.3×.** It applies only where a missed defect
  costs a leak, a data loss or a migration every user pays for. On Claude
  Code the rung after that (xhigh) is 1.12× for 2.2× cost. Max adds nothing
  over xhigh at double the cost.
- **The agent judges, the engine bounds.** The engine never reads the diff.
  The agent has just built the change and knows what it touched. The consent
  covers two rungs, so a wrong judgement costs at most 1.3×.

## Consequences

- A cheap builder pays for one handoff at harden, in exchange for a review on
  a different, stronger model in a fresh context.
- An Apply-time harden on a thread with an open change never hands off: a
  handoff withdraws the pending change.
- `HandoffCheckpointReached` records `default_floor`, `floor_rung` and
  `risk_rung`, which the consent check reads back.
- `phase-commit` and plan approval are unchanged. The plan card keeps its own
  raise clause (ADR 0499).

## Alternatives considered

- **Stay on the builder's rung.** Cheapest, but the review shares the
  builder's blind spots, and Sonnet medium builders (0.84×) lose most.
- **Harden on Opus 5.5 high or above by default.** 1.08× for 1.3× cost and
  1.3× time on every call, suite polling included. `/code-review`'s effort
  argument is the cheaper lever for a risky change.
- **A deterministic path rule for the risk rung** (`migrations/`,
  `api/actor.rs`). It misses risk in an ordinary file and flags trivial edits
  in a risky one.
- **The engine declares the floor at `pre-harden`.** ADR 0505 keeps
  checkpoint names out of the engine, so the flow declares it instead.
