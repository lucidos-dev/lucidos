# 0499: A successor offer follows effort, and reaches repo checkpoints

- **Status**: Accepted
- **Amends**: 0486
- **Date**: 2026-10-16

## Context

ADR 0486 made the plan card's new-thread offer the measured context's call. A
thread under `HANDOFF_OFFER_TOKENS` never offered one. Two gaps followed.

- A thread running at `medium` or `low` could not hand a hard plan to a builder
  at `high`. Claude Code takes effort only at spawn, and a top-level change
  rewrites the conversation cache. A successor starts a fresh context anyway,
  so it is the cheap place to change effort.
- The offer existed only at plan approval, through a skill that external repos
  do not ship. A repository could not name its own points to offer a handoff.

## Decision

A small thread may offer a successor at `high` when the plan is hard or long.
The CLI prints the raise clause only when the engine says the thread is small
and the thread's recorded effort is `low` or `medium`. An unknown size or an
unknown effort prints none. The agent judges only the plan. The raise keeps
this thread's backend and model, and never names `xhigh` or `max`.

A *handoff checkpoint* is any point where the agent may offer a successor. Plan
approval is the built-in one. A repository's `CLAUDE.md` or `AGENTS.md` may
name more. At one, the agent runs `lucidos successor offer` and follows what it
prints. One engine prompt rule reaches every coding-agent session, external
repos included.

Both checkpoint kinds print from one `card_shape`. Every successor option keeps
the `Implement in a new thread` prefix, so ADR 0493's owner consent applies.

`POST /api/v1/claude-code/control` now validates a `set_reasoning_effort`
value, as every other effort entry point does.

## Rationale

The CLI compares the recorded effort, as it compares the size, so the agent
never weighs tiers or token counts itself (ADR 0486). Raising effort in a
successor costs one cache rewrite that the successor pays anyway.

A repository knows its own natural pauses, such as the end of a design phase.
The agent reads them from the instructions it already loads, so there is no new
file format and no engine detection.

## Consequences

- A small thread at `medium` gets an extra card shape when the agent judges the
  plan needs it. Whether the plan needs it is a judgement no test can check.
- The prompt rule costs 108 to 141 bytes in every coding-agent prompt.
- A thread at `high` or above, or with no recorded effort, never sees a raise.
- ADR 0497's cheaper pick still comes first on a small card. The raise applies
  only when no pick is cheaper, and both reach repo checkpoints.

## Alternatives considered

- **Switch effort inside the thread.** Claude Code 2.1.300 has no per-message
  effort beta, so every switch busts the messages cache.
- **A checkpoint file format.** More machinery than a line in `CLAUDE.md`.
- **Let the agent compare effort tiers.** The CLI already owns that kind of
  comparison, and the agent would drift.
