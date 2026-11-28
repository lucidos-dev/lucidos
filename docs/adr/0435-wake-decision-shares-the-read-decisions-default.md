# 0435: The wake decision shares the read decision's OpenAI Decisions default and its fallbacks

- **Status**: Accepted
- **Date**: 2026-10-11
- **Amends**: [0433](0433-turn-end-decisions-are-judgment-sites.md), [0434](0434-sub-threads-wake-parents-only-when-needed.md)
- **Amended by**: [0439](0439-wake-decision-measured-on-a-synthetic-set.md), which measures the wake decision on a synthetic set; [0472](0472-trigger-runs-decide-their-read.md), which leaves the wake decision alone on this default

Record: `docs/plans/2026-10-11-wake-decision-defaults-to-openai-decisions.md`.

## Context

ADR 0433 made OpenAI Decisions the default for the read decision, and said no
other site has a default. ADR 0434 added the wake decision and kept it on its
chat model until the user picks a backend. The user decided, in the planning
thread for `docs/plans/2026-10-11-sub-threads-wake-parents-only-when-needed.md`,
that both turn-end decisions follow one rule.

## Decision

- **Both turn-end decisions share one default.** `TURN_END_DECISION_DEFAULT`
  in `llm/judgment/select.rs` is the one definition. `default_endpoint`
  returns it for `ReadDecision` and `WakeDecision`, and Settings reads its
  generated copy, `TURN_END_DECISION_DEFAULT_ENDPOINT`.
- **The same rule for each.** An unset `judgment_wake_decision` runs on
  OpenAI Decisions while the running chat provider serves OpenAI, its switch
  is on, and `model_wake_decision` is unset. An explicit `chat` or a pinned
  `model_wake_decision` keeps chat. One engine helper resolves both sites
  through `system_one_or_default_for`.
- **The wake decision's fail-safe stands.** A failed OpenAI call falls back to
  `model_wake_decision`. A failed chat call, a missing answer or an empty reply
  still wakes the parent.

## Rationale

- **One rule is easier to hold.** Both are small typed yes or no questions at
  the same turn end. A user who sees one on OpenAI Decisions expects the other
  there too.
- **The read-decision numbers carry over in kind, not in score.** ADR 0433
  measured OpenAI Decisions at 0 missed must-reads on 20, about 15 times
  faster and 20 times cheaper than chat. The wake decision asks a different
  question, so those scores are no measure of it.
- **No wake-decision eval exists.** `lucidos-eval` has a read-decision check
  only. The default rests on the user's decision, not on a wake score.
- **The fail-safe bounds the risk.** A wrong yes costs one parent turn. A wrong
  no files a report the parent still reads on its next turn, and a failure
  always wakes.

## Consequences

- A workspace with OpenAI configured, through a key or a Codex login, sends
  each undecided child report to OpenAI Decisions. The Wake decision control
  shows it, and picking a chat model there writes `chat`.
- ADR 0433's "no other site has a default" now reads "no site outside the turn
  end". ADR 0434's "stays on its chat model until the user picks" no longer
  holds.
- The mock provider serves nothing, so a test engine never reaches OpenAI.
- Before changing this default again, build a wake-decision eval: a synthetic
  set of child reports, each labelled must-act or filed.

## Alternatives considered

- **Keep the wake decision on chat.** Lost: the user chose one rule for both
  turn-end decisions, and two rules for siblings invite surprise.
- **Copy the read decision's default into a second constant.** Lost: two
  copies drift, and the rule is one decision (one definition per value).
- **Wait for a wake-decision eval first.** Lost on the user's call. The
  fail-safe makes a wrong answer cheap, and the eval stays a precondition for
  the next change.
