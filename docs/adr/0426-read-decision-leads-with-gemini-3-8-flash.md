# 0426: The read decision leads with Gemini 3.8 Flash, then GPT-5.6 Luna, measured on real and synthetic turns

- **Status**: Accepted
- **Date**: 2026-10-11
- **Amends**: [0417](0417-turn-end-gate-enforces-valid-states.md)
- **Amended by**: [0472](0472-trigger-runs-decide-their-read.md), which retunes the question for trigger runs and pushes

## Context

The user kept finding threads in Review that needed no reading. In a
maintainer workspace over two days, the turn-end gate forced 88 read
decisions and said yes to 70 of them. Agents that decided for themselves said
yes to 15 of 123.

Three things went wrong in the forced call:

- **It never saw the user's answer.** A question answer resumes the same turn,
  so the call read the turn's first event. A trigger run's reply that only
  confirmed the user's answer read like the run's findings.
- **Its prompt let detail through.** It already called an applied change a
  no, but a long, friendly report of one still drew a yes. So did kickoffs.
- **Its model was the cheapest one.** `gemini-3-flash-preview` at `none`, the
  shared background default, which no one had measured on this task.

## Decision

- The forced call reads a question the user answered inside the turn,
  between the opener and the reply.
- Its prompt asks whether the reply holds something new the user would miss.
  It names a confirmation of what the user said, a ready or applied change,
  and a kickoff as a no.
- The read decision leads with `gemini-3.8-flash`, then `gpt-5.6-luna`, at
  the task's existing `none` tier. `gemini-3-flash-preview` stays its catalog
  default, behind them, then the shared fallbacks.

## Rationale

`lucidos-eval read-decision` scores a candidate through the gate's own prompt,
tool and system prompt. Two sets were used.

**Real turns.** The workspace's 69 forced decisions on chat and trigger
threads, labelled by hand: 12 worth reading, 57 not. The set stays private,
since it holds a user's conversations. Its in-flight counts were not recorded,
so progress turns read harder than they do live.

| Model, tier | Missed must-read | Noise passed | p50 ms | p95 ms |
|---|---|---|---|---|
| Old prompt, Gemini 3 Flash, none (as recorded) | 2/12 | 43/57 | | |
| Gemini 3 Flash, none | 0/12 | 31/57 | 1006 | 1440 |
| Gemini 3 Flash, low | 0/12 | 29/57 | 1077 | 4783 |
| Gemini 3.5 Flash, none | 0/12 | 21/57 | 1434 | 7392 |
| **Gemini 3.8 Flash, none** (3 runs) | 0/12 | 9-10/57 | 3164 | 6335 |
| Gemini 3.8 Flash, low (3 runs) | 0/12 | 9-10/57 | 3323 | 5810 |
| Claude Haiku 4.5, none | 0/12 | 35/57 | 416 | 525 |

The same set, for an install with only OpenAI configured, where Gemini 3.8
Flash is out of reach:

| Model, tier | Missed must-read | Noise passed | p50 ms | p95 ms |
|---|---|---|---|---|
| GPT-5.4 mini, none (2 runs) | 1/12, then 0/12 | 33-34/57 | 765 | 1047 |
| GPT-5.4 mini, low | 0/12 | 41/57 | 959 | 1918 |
| **GPT-5.6 Luna, none** (2 runs) | 0/12 | 28-30/57 | 1037 | 1407 |
| GPT-5.6 Luna, low | 0/12 | 31/57 | 899 | 1331 |
| GPT-6.1 Sol, none | 1/12 | 18/57 | 1571 | 2807 |

**Synthetic turns.** `eval/read-decision/turns.toml`, 53 turns that ship
publicly: 20 must-read, 33 noise. The first 40 saturated, since every model
scored 40 of 40 once the prompt was sharpened. The last 13 copy the shape of
the real noise that still passed.

| Model, tier | Missed must-read | Noise passed | p50 ms | p95 ms |
|---|---|---|---|---|
| Gemini 3 Flash, none | 0/20 | 6/33 | 914 | 1253 |
| Gemini 3.8 Flash, none | 0/20 | 1/33 | 2858 | 4192 |

- **The answer and the prompt matter more than the model.** On the same
  model, missed must-reads went from 2 to 0 and noise from 43 to 31.
- **Gemini 3.8 Flash then cuts noise by two thirds**, with no missed
  must-read. No other candidate came close.
- **`none` over `low`.** They tied on noise, 28 passes each over three runs.
  `low` had the lower p95 and the higher p50, both within half a second. The
  tier also binds the fallback models, where `low` was never measured here and
  is slow on Haiku 4.5 (ADR 0406). So the task keeps the `none` it had.
- **GPT-5.6 Luna is the second lead.** On an OpenAI-only install it missed no
  must-read in two runs, where GPT-5.4 mini, the shared fallback, missed one.
  GPT-6.1 Sol was quieter but missed a must-read, which weighs more.
- **Leads, not a new catalog default.** That is how the memory task carries
  its measured model (`purpose_lead`). An install whose providers serve
  neither lead resolves as before.

## Consequences

- An undecided turn's end now waits about 3 seconds at p50 for the decision,
  up from 1, and about 6 seconds at p95. The gate runs before `ResponseGenerated`, so the turn reads
  finished that much later. The reply has already streamed by then.
- Each decision costs about 1,150 input tokens and 100 to 160 output tokens,
  up from 14 output tokens: the model thinks briefly even at `none`.
- A stored `model_read_decision` is untouched, whatever it names.
- An OpenAI-only install decides on GPT-5.6 Luna. Its noise stays near the
  old default's, about half of the replies that need no reading.
- An Anthropic-only install still decides on Claude Haiku 4.5, measured only
  at `none`: no missed must-read, and the most noise of any candidate.
- The real set is one workspace's two days, labelled by one person. It is
  evidence, not proof. Rerun both sets before any later change.

## Alternatives considered

**Keep the model, fix only the answer and the prompt.** It removes the
missed must-reads, but 31 of 57 noise replies still land in Review, the
complaint that started this.

**Gemini 3.8 Flash at `low`.** Equal on noise and missed reads, and within half a second on latency, but it would move every fallback model to `low` as well.

**Raise Gemini 3 Flash's tier.** At `low` it passed 29 of 57, barely better
than `none`, for four times the p95.

**Claude Haiku 4.5.** The fastest, and the noisiest at 35 of 57.

**Make the agent always decide, and drop the forced call.** Agents decide
well when they do (12% yes), but many turns end without deciding. The forced
call exists so that no turn ends undecided (ADR 0417).
