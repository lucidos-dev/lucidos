# 0486: The new-thread plan card's implementer is the planning agent's judgement, not a rule or a plan score

- **Status**: Accepted
- **Date**: 2026-10-15

## Context

On a large planning thread the plan-approval card leads with `Implement in a
new thread`. The successor always ran on the planning thread's backend, model
and effort. A user planning on Opus asked to build on Sonnet, and the only way
was a Sonnet subagent inside the heavy planning thread.

So the card now recommends an *implementer* (backend, model and reasoning
effort), and `lucidos successor spawn` carries the choice to the successor.
Something has to pick the recommendation.

## Decision

The planning agent picks it, by its judgement of the plan, and says why in one
line. `lucidos planned mark` gives it this thread's implementer and a rule of
thumb. Whether to offer a new thread at all stays the measured context's call.

The recommended effort follows from the model. A cheaper model on a fully
specified plan gets `medium`. A hard or long plan, and the planning model, get
`high`. The agent never recommends `xhigh` or `max` for a builder.

## Rationale

Only the agent has read the plan. The deciding question is how much judgement
the plan leaves to the builder. A plan with settled decisions, named files and
a verification per invariant suits a cheaper model. A plan whose phases still
hold design choices needs the planning model. That is a reading of the text,
not a count.

The two decisions differ in kind. Context size is a number the engine
measures, so a rule over it is exact and the agent may not override it. Plan
difficulty has no such number.

Effort is a real cost lever, possibly as large as the model. A coding agent's spend is mostly
cache reads and writes: about 96% in one dev workspace over thirty days. Every
call reads the whole context again, and effort sets how many calls a turn
makes. Anthropic's effort docs say lower effort makes fewer tool calls. In that
workspace, `medium` turns made about half the calls of `high` ones.

The effort rule follows Anthropic's own guidance. For Sonnet 5.5 on agentic
coding, it says to start at `medium` for well-specified tasks and `high` for
harder or longer ones. That is the plan a cheaper builder gets. Above `high`,
benchmarks show small gains for several times the cost.

The card always names this thread's implementer beside the recommendation,
and a typed reply can name any other. A poor pick costs one tap.

## Consequences

- The card text in `planned.rs` carries the rule of thumb and the label shape.
  It is prose the agent follows, so no test can check the pick itself.
- The engine validates the chosen model and effort against the backend's
  picker, so a mistaken id is refused with the offered list, never swapped.
- Recommendations may vary between agents and models. That is accepted.

## Alternatives considered

- **A fixed model rule** (always the cheaper sibling, for example Sonnet under Opus).
  Predictable, but blind to the plan: it recommends Sonnet for a plan that
  still holds open design choices.
- **`high` effort for every builder.** Briefly adopted on the reading that
  effort changes only output tokens, a small share of spend. That missed the
  call count, which multiplies the cache bill.
- **A heuristic scored from the plan file** (phase count, open questions,
  invariant count). Deterministic, but plan shape is a weak proxy for
  difficulty, and an agent writing the plan would learn to shape it.
