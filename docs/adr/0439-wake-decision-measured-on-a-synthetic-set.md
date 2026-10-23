# 0439: The wake decision measured: OpenAI Decisions and Jev miss no must-wake, every chat model in its chain does

- **Status**: Accepted
- **Date**: 2026-10-12
- **Amends**: [0435](0435-wake-decision-shares-the-read-decisions-default.md)
- **Amended by**: [0448](0448-wake-decision-forced-with-a-measured-lead.md), which forces the chat call, runs it only on measured models and sharpens the question

Record: `docs/plans/2026-10-12-wake-decision-eval.md`.

## Context

ADR 0435 put the wake decision on OpenAI Decisions with no wake score, and
named a wake-decision eval as the precondition for the next change. The user
asked for one in the planning thread for
`docs/plans/2026-10-11-sub-threads-wake-parents-only-when-needed.md`.

The costly error is a missed must-wake: a report filed while the parent waits
for it. The parent then waits for a wake that never comes. A wrong wake costs
one parent turn.

## Decision

- **`lucidos-eval wake-decision` scores the gate itself.** It calls
  `decide_wake_for_eval`, which runs the gate's own `wake_state`,
  `wake_questions` and threshold. Its chat backend is
  `wake_decision_chat_judge`, the constructor the gate uses. A unit test holds
  that the eval and the gate send one state and one question set.
- **The set is public and synthetic.** `eval/wake-decision/turns.toml` holds
  31 child reports: 16 must-wake and 15 file. It covers pipelines, fan-in,
  Home starting work for the user, research the user reads directly, ready
  changes, problems, and the user talking to the child.
- **No default changes.** OpenAI Decisions stays the default. The chat chain's
  misses go to a follow-up with its own plan.

## Rationale

Three runs per backend. Cost uses the mean tokens and October 2026 list
prices.

| Backend, tier | Missed must-wake | Noise woken | p50 ms | p95 ms | Tokens in / out | Cost per 1,000 |
|---|---|---|---|---|---|---|
| OpenAI Decisions (`gpt-6-luna`) | 0, 0, 0 of 16 | 7, 7, 7 of 15 | 133-138 | 359-497 | 494 / 0 | $0.05 |
| Jev through OpenRouter | 0, 0, 0 of 16 | 11, 11, 11 of 15 | 303-325 | 452-487 | 637 / 23 | $0.03 |
| Jev direct (TypeSafe) | not run | not run | | | | |
| Chat: Gemini 3 Flash, none | 0, 0, 1 of 16 | 5, 8, 5 of 15 | 874-978 | 1270-2979 | 667 / 18 | $0.39 |
| Chat: GPT-5.4 mini, none | 3, 0, 1 of 16 | 3, 3, 3 of 15 | 847-942 | 1339-1613 | 630 / 22 | $0.57 |
| Chat: Claude Haiku 4.5, none | 1, 1, 0 of 16 | 5, 2, 5 of 15 | 3561-3736 | 4390-4684 | 1215 / 345-381 | $3.02 |

Prices per million tokens: OpenAI Decisions $0.10 in with free output, a beta
price. Jev $0.042 in with free output, plus OpenRouter's 5.5% credit fee.
Gemini 3 Flash $0.50 and $3.00. GPT-5.4 mini $0.75 and $4.50. Haiku 4.5 $1
and $5.

- **Both decision models miss nothing.** OpenAI Decisions and Jev gave the same
  answers in all three runs.
- **Every chat model in the chain misses must-wakes, each confidently.** Gemini
  3 Flash filed the last sibling of a combine at 0.0. GPT-5.4 mini filed an
  export its parent needed for the next step twice, at 0.12 and 0.15. It also
  filed a ready migration the parent had to schedule, and a fix it could not
  make. Haiku 4.5 filed a stalled download twice.
- **The decision models wake too often.** OpenAI Decisions wakes every early
  fan-in sibling, and three of four Home reports. Jev wakes every Home,
  research and ready-change report, which is the noise ADR 0434 set out to
  remove.
- **The question invites that noise.** Its yes side says the parent "waits for
  this result to ... combine results". Read literally, that covers an early
  sibling too. The rule that a combine acts on the last report sits in the
  instructions only.
- **The chat backend thinks.** It is unforced, unlike the read decision's.
  Haiku 4.5 wrote about 360 output tokens per decision, which is why it is
  slow and costly.
- **Jev direct did not run.** TypeSafe answered 402: the account has no API
  credits. The OpenRouter row asks the same model.
- **One label moved after the first runs.** The first runs labelled a child
  ending with "I'll fetch the rest later" as file. A child holding an event wait never reaches
  the call, so that child has stalled, and it is now must-wake. The table
  scores the recorded answers against the corrected label.

## Consequences

- OpenAI Decisions keeps the default, now on a score: no missed must-wake in
  three runs.
- The chat chain is a real path. It runs when OpenAI is not served, and when
  an OpenAI Decisions call fails, so a failure there can still lose a wake.
- A follow-up, with its own plan, takes the chat chain's misses and the
  question's noise.
- The set is small, synthetic and labelled by one agent. It is evidence, not
  proof. Rerun it before any later change to the wake decision.

## Alternatives considered

- **Change the defaults here.** Lost: the user asked for measurement only, and
  each fix (a lead model, a forced chat call, a sharper question) is its own
  decision.
- **A copy of the gate's prompt in the eval.** Lost: a copy drifts, and the
  eval would then score a question the engine no longer asks.
- **Include failure finishes in the set.** Lost: the gate wakes on them without
  a call, so there is nothing to score.
