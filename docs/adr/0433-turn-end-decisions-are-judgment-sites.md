# 0433: Turn-end decisions are judgment sites, with OpenRouter and OpenAI Decisions as System One rows

- **Status**: Accepted
- **Date**: 2026-10-11
- **Amends**: [0363](0363-any-system-one-endpoint-and-a-chat-judgment-provider.md), [0417](0417-turn-end-gate-enforces-valid-states.md), [0426](0426-read-decision-leads-with-gemini-3-8-flash.md)
- **Amended by**: [0435](0435-wake-decision-shares-the-read-decisions-default.md), which gives the wake decision the same default; [0472](0472-trigger-runs-decide-their-read.md), which moves the read decision's default to chat

Record: `docs/plans/2026-10-11-sub-threads-wake-parents-only-when-needed.md`,
Phases 2 and 3.

## Context

The read decision was one forced chat call with `request_read` as its only
tool (ADR 0417, 0426). The user asked for decision models to make it, and to
make the new wake decision: TypeSafe's Jev, OpenAI's Decisions API, or a
decision model through OpenRouter.

The judgment layer (ADR 0363) already asks typed questions on any backend, with
a per-site preference that defaults to chat. Two backends were missing.
OpenRouter serves Jev on the System One wire. OpenAI's Decisions API is its
own wire: `POST /v1/decisions` on `gpt-6-luna`, in beta since 6 October 2026.

## Decision

- **Two new System One rows.** `openrouter` posts the System One body to
  OpenRouter, asking `~typesafe/jev-latest` unless
  `system_one_openrouter_model` names another model. `openai` speaks the
  Decisions API through `OpenAiDecisionsProvider`. Each reuses its chat
  provider's key chain and `provider_enabled_*` switch. A site's pick is now a
  boxed `JudgmentProvider`, since the rows speak two wires.
- **The read decision is `JudgmentSite::ReadDecision`.** It asks one Noul
  question, `read`, over the turn's text. The old system prompt's meaning lives
  in the question's instructions and criteria. `READ_THRESHOLD` is 0.5, and a
  missing answer or a failed call records a read request.
- **Its chat backend stays forced.** `model_read_decision` answers through
  `ChatJudgmentProvider::forcing_the_answer`, so a reply always carries the
  answer and runs without thinking, as the forced call did.
- **The read decision defaults to OpenAI Decisions.** An unset
  `judgment_read_decision` runs on the `openai` row while the running chat
  provider serves OpenAI and its switch is on, else on chat. A pinned
  `model_read_decision` counts as a choice of chat, so a model kept local for
  privacy never moves. The user chose this after the numbers below. It is the one exception to ADR 0363's rule
  that a stored key moves no site. An explicit `chat` keeps chat, and no
  other site has a default.

## Rationale

`lucidos-eval read-decision --backend chat|jev|openrouter|openai` scores each
backend through the gate's own state, question and threshold. It ran on the
public synthetic set, `eval/read-decision/turns.toml`: 20 must-read turns and
33 noise turns. One run each.

| Backend | Missed must-read | Noise passed | p50 ms | p95 ms | Tokens in / out | Cost per 1,000 |
|---|---|---|---|---|---|---|
| Before: forced `request_read`, Gemini 3.8 Flash, none | 0/20 | 1/33 | 2943 | 4524 | 568 / 100 | $0.80 |
| Chat: judgment site, Gemini 3.8 Flash, none | 0/20 | 1/33 | 4086 | 5319 | 722 / 191 | $1.26 |
| Jev direct (TypeSafe) | not run | not run | | | | |
| Jev through OpenRouter (`jev-1.13`) | 0/20 | 2/33 | 349 | 529 | 709 / 20 | $0.03 |
| OpenAI Decisions (`gpt-6-luna`) | 0/20 | 2/33 | 239 | 1348 | 571 / 0 | $0.06 |

Cost uses the mean tokens and the October 2026 list prices. Gemini 3.8
Flash is $0.75 in and $3.75 out per million, an introductory rate that
doubles on 1 January 2027. Jev is $0.042 in with free output, plus
OpenRouter's 5.5% credit fee. OpenAI Decisions is $0.10 in with free output,
a beta price.

- **The read decision did not get weaker.** On chat it misses no must-read and
  passes the same one noise turn as before (invariant I7).
- **Chat got slower.** p50 rose by about 1.1 seconds. The judgment question
  carries the generic answer-tool prompt and longer criteria, and Gemini 3.8
  Flash wrote about twice the output tokens.
- **Both decision models tie each other.** Each missed nothing and passed two
  noise turns: an acknowledgement and a progress report. Each answered 12 to 17
  times faster than chat, at 20 to 40 times less cost.
- **They err differently.** OpenAI Decisions passed its two noise turns at
  0.98 and 0.92, so it is confidently wrong there. Jev's noise sits closer to
  the threshold, between 0.35 and 0.74.
- **The threshold is not the lever.** Scored offline from the recorded
  probabilities, chat and OpenAI Decisions give the same result anywhere from
  0.3 to 0.9. OpenRouter's Jev drops its noise to 0 at 0.9, but then misses 4
  must-reads.
- **Jev direct did not run.** TypeSafe refused every call with 402: the
  account has no API credits. The OpenRouter row asks the same Jev version.

## Consequences

- A workspace with OpenAI configured sends every undecided reply to OpenAI
  Decisions with no step of its own. A Codex login alone is enough. The Read decision control shows it, and picking a chat model there
  writes `chat` and keeps it.
- Undecided turns end about 4 seconds sooner at p50, and a decision costs
  about $0.06 per thousand instead of $1.26.
- One more noise turn in 33 reached Review on the synthetic set. Both noise
  turns OpenAI passed came with high confidence, so raising the threshold
  would not remove them.
- The mock provider serves no backend, so a test engine never reaches OpenAI
  through the default.
- The wake decision (Builder B's phases) gets the same rows through
  `for_site(system_one_for(..), chat)`.
- The forced `request_read` schema is gone. `request_read` stays as the
  agent's own tool, and the forced tool is now the judgment answer tool.
- The synthetic set is small and was run once per backend. Rerun it, and the
  private real-turn set, before changing the default again.

## Alternatives considered

- **Keep chat as the default.** Lost: one fewer noise turn on 33, for about
  15 times the latency and 20 times the cost.
- **Default to Jev through OpenRouter.** Cheapest and steady on latency, with
  the same quality. Lost to OpenAI on the user's pick: an OpenAI key is more
  common here, including through a Codex login, and its p50 is lower.
- **Keep the forced `request_read` call beside the judgment site.** Lost: two
  paths for one decision, and only one of them could run on a System One row.
- **An unforced chat judgment.** Lost: a model could then answer in text or
  think at length. Forcing keeps ADR 0426's measured behaviour.
- **Make OpenAI Decisions a chat provider model.** Lost: it answers typed
  questions only, so it belongs in the judgment layer (ADR 0220).
