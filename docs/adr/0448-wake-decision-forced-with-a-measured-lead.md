# 0448: The wake decision runs forced, only on measured models, and reads the siblings in words

- **Status**: Accepted
- **Date**: 2026-10-12
- **Amends**: [0439](0439-wake-decision-measured-on-a-synthetic-set.md)

Record: `docs/plans/2026-10-12-wake-decision-chat-fallback-and-question.md`.

## Context

ADR 0439 scored the wake decision with `lucidos-eval wake-decision`. OpenAI
Decisions and Jev missed no must-wake. Every chat model in the
`model_wake_decision` chain did, each with a confident no. That chain runs
when OpenAI is not served, and when an OpenAI Decisions call fails. So a
parent could wait for a wake that never comes.

ADR 0439 also found the question inviting noise. OpenAI Decisions woke every
early fan-in sibling and three of four Home reports. Jev woke every Home,
research and ready-change report.

## Decision

- **The chat call is forced**, as the read decision's is.
  `wake_decision_chat_judge` adds `forcing_the_answer()`. A model that always
  thinks refuses a forced call, the call fails, and a failed call wakes.
- **Unpinned, the wake decision runs only on a model the eval cleared.** Its
  lead is `gemini-3.8-flash`, `gpt-6.1-sol`, `gpt-5.6-luna`, then its catalog
  default `gemini-3-flash-preview`. It never takes the shared fallbacks or
  `LUCIDOS_EXTRACTION_MODEL`.
- **With no cleared model reachable, no chat model is asked.** A System One
  pick still answers alone. Without one, the parent wakes with no call. A
  pinned `model_wake_decision` runs whatever it names. Settings says so on the
  row (`asks_no_model` in `GET /api/v1/models/background`).
- **The state reads the siblings in words.** `parents_other_children` replaces
  the bare `other_children_still_running` count. At 0 it says the child is the
  last to report.
- **The question is sharper.** A combining parent acts on the last report. A
  result the parent needs for its next step wakes it even while siblings
  work. The yes side names a parent that will use, check, relay, apply or
  schedule the result. The no side names a parent waiting to combine with
  reports still to come, and a parent that handed the user to the child.
- **The threshold stays 0.3**, and OpenAI Decisions stays the default.

## Rationale

### The set grew, in two held-out batches

`eval/wake-decision/turns.toml` now holds 51 reports: 27 must-wake, 24 file.

- **The original 31** from ADR 0439 tuned the question.
- **Ten held-out cases** were written before any question draft. They caught
  a regression in a draft, so they then tuned the next one.
- **Ten more held-out cases** were written before that next draft, and never
  tuned anything.

### The final runs

Three runs per backend on all 51, under the shipped question and state. Cost
uses the mean tokens and the list prices below.

| Backend, tier | Missed must-wake | Noise woken (of 24) | Of the original 15 | Held-out batch 2 | p50 ms | p95 ms | Tokens in / out | Cost per 1,000 |
|---|---|---|---|---|---|---|---|---|
| OpenAI Decisions | 0, 0, 0 of 27 | 9, 9, 9 | 5, 5, 5 (was 7) | 0 missed, 3 noise | 250-256 | 361-507 | 595 / 0 | $0.06 |
| Jev through OpenRouter | 0, 0, 0 | 17, 17, 17 | 11, 11, 11 (was 11) | 0 missed, 4 noise | 302-315 | 378-458 | 741 / 23 | $0.03 |
| Chat: Gemini 3.8 Flash | 0, 0, 0 | 0, 0, 0 | 0, 0, 0 | 0 missed, 0 noise | 4423-5069 | 6564-9675 | 768 / 227 | $1.43 |
| Chat: GPT-6.1 Sol | 0, 0, 0 | 1, 0, 0 | 0, 0, 0 | 0 missed, 0-1 noise | 1809-1912 | 2746-4279 | 731 / 26 | $1.73 |
| Chat: GPT-5.6 Luna | 0, 0, 0 | 20, 21, 21 | 13, 14, 13 | 0 missed, 4-5 noise | 1153-1179 | 1367-1938 | 731 / 22 | $0.17 |
| Chat: Gemini 3 Flash | 0, 0, 0 | 17, 17, 18 | 11, 11, 12 | 0 missed, 4 noise | 915-992 | 1508-1584 | 768 / 18 | $0.44 |
| Chat: GPT-5.4 mini | 0, 1, 1 | 14, 16, 18 | 9, 9, 12 | 0-1 missed | 872-885 | 1195-1377 | 731 / 22 | $0.65 |
| Chat: Claude Haiku 4.5 | 2, 1, 1 | 15, 15, 15 | 10-11 | 0 missed, 3-4 noise | 468-482 | 561-757 | 1416 / 38 | $1.61 |

Every chat row runs at `none`. Gemini 3.8 Flash and GPT-6.1 Sol answer it as
`low`, the lowest tier each serves.

Prices per million tokens, in and out:

- OpenAI Decisions: $0.10, output free (a beta price).
- Jev: $0.042, output free, plus OpenRouter's 5.5% fee.
- Gemini 3.8 Flash: $0.75 and $3.75 (introductory).
- GPT-6.1 Sol: $2 and $10.
- GPT-5.6 Luna: $0.20 and $1.20.
- Gemini 3 Flash: $0.50 and $3.00.
- GPT-5.4 mini: $0.75 and $4.50.
- Claude Haiku 4.5: $1 and $5.

### What each lever did

- **Forcing removed the thinking, and the misses with it, for most models.**
  On ADR 0439's question and the first 41 cases, forced: Haiku 4.5 wrote 38
  output tokens instead of about 360, at 478 ms instead of 3.6 s. It and
  Gemini 3 Flash missed nothing in one run. GPT-5.4 mini still missed 3.
- **No question draft helped every backend at once.** The small chat models
  answer near 0 or 1 and flip whole groups of cases between drafts. OpenAI
  Decisions is deterministic but sensitive. Changing only the no side made it
  file a last sibling.
- **A bare sibling count misled the decision models.** With the count as a
  number, OpenAI Decisions told a last sibling from an early one by chance:
  0.04 on one, 0.89 on another. Read as a sentence, it scored every fan-in
  case 1.00 or 0.00. Jev read a negative sentence ("None ... is still
  working") as no work to combine, so the 0 case is worded positively.
- **"Another report will follow" overreached.** It made OpenAI Decisions,
  Gemini 3 Flash, Haiku 4.5 and Luna file an early sibling whose result the
  parent needed at once. The first held-out batch caught it. The shipped
  sentence states the count and nothing more.
- **Unforced Haiku 4.5 does no better.** On the shipped question it missed one
  stalled child in each of three runs, at 3.9 s per call.

### Why the lead and why only measured models

- **Gemini 3.8 Flash first, GPT-6.1 Sol second**, by noise then p50. Both
  missed nothing and woke at most once in 72 file decisions. Together they
  cover Vertex, OpenAI and OpenRouter installs. GPT-5.6 Luna follows, so an
  OpenAI install whose Sol answers not-found moves to a model that cleared
  the bar.
- **No cheap Claude model clears it.** Haiku 4.5 misses about one must-wake
  per run, forced or not. Claude Sonnet 5.5 and Haiku 5.5 always think, so
  they refuse a forced call. Sonnet 5 can be forced and missed 7 of 22 on one
  screening run.
- **The user chose waking over Haiku 4.5** for an install with no cleared
  model: a wrong wake costs one parent turn, and a missed one can strand the
  parent.

## Consequences

- **An Anthropic-only install wakes the parent for every undecided report**,
  as do Local, xAI and OpenCode Free installs. That is the noise ADR 0434 set
  out to remove, traded for no missed wake. A pinned `model_wake_decision`, or
  a System One pick, gets filing back.
- **An engine on the mock model asks no model either**, since its aux calls
  resolve to the chat model. Its undecided child reports always wake.
- **A chat fallback can take seconds.** Gemini 3.8 Flash answers at about 4.5
  s p50 and up to 9.7 s p95, within the call's deadline. It runs only where
  OpenAI Decisions is not served, or after it failed.
- **OpenAI Decisions wakes less**: 5 of the original 15 file reports, down
  from 7. Jev's noise is unchanged at 11 of 15. It now sits on Home, research,
  ready-change and user-in-child reports instead of early siblings, at
  probabilities between 0.3 and 0.7. Only the threshold would move that.
- **GPT-5.4 mini and Claude Haiku 4.5 stay the shared fallbacks** for every
  other background task. The wake decision alone skips them.
- **The set is still synthetic and labelled by one agent.** The second held-out
  batch is evidence the question generalises, not proof. Rerun every backend
  before any later change to the question, the state or the lead.

## Alternatives considered

- **Ship Haiku 4.5 on Anthropic-only installs.** Lost on the user's call: it
  missed 2, 1 and 1 must-wakes in three runs.
- **Keep ADR 0439's question and fix only the chat chain.** Lost: the
  question's noise on OpenAI Decisions, the default, was the second half of
  the task, and the sibling count was a real ambiguity.
- **Name the count field in the question instead of wording it.** Tried in
  two drafts. OpenAI Decisions still missed last siblings.
- **Lower or raise the threshold.** Out of scope. The results files keep each
  probability, so a threshold can be scored offline later.
- **Unforced chat for models that always think.** Lost: it costs a second or
  more of thinking per call, and unforced Haiku 4.5 still missed.
- **GPT-6.1 Sol first.** It is 2.5 times faster than Gemini 3.8 Flash, but
  woke once where Gemini 3.8 Flash never did. The two cover different
  providers, so the order matters only on an install with both.
