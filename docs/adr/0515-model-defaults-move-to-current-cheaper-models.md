# 0515: The model defaults move to current, cheaper models

- **Status**: Accepted
- **Date**: 2026-10-18
- **Supersedes**: the default clauses of [0406](0406-command-judge-defaults-to-gemini-3-flash.md)
- **Amended by**: [0533](0533-trigger-model-default-separate-from-chat.md), which returns the chat default to Opus 5.5 and gives unpinned triggers Sonnet 5.5
- **Amends**: [0377](0377-memory-tasks-lead-with-their-measured-models.md), [0426](0426-read-decision-leads-with-gemini-3-8-flash.md), [0448](0448-wake-decision-forced-with-a-measured-lead.md)

Record: `docs/plans/2026-10-18-model-defaults-audit.md`.

## Context

A dev-workspace thread got no title because its title model hung. That
prompted an audit of every model default. Most defaults named a model a
newer, cheaper one had superseded, and one, `gemini-2.5-flash-lite`, had
retired on Vertex.

The maintainer set the rule:

- newer and cheaper models are our defaults, and no default names a
  superseded model;
- a superseded model retires to a successor (ADR 0418);
- each default carries the effort public benchmarks and provider guidance
  point to.

## Decision

| Purpose | Default | Effort |
|---|---|---|
| Chat (`chat_model`, and `LUCIDOS_MODEL` unset) | `claude-sonnet-5-5` | `high` |
| `lucidos-eval` | `claude-opus-5-5` | `medium` |
| Background default (`BACKGROUND_MODEL`) | `claude-haiku-5-5` | `none` (task tiers stay) |
| Command judge (`COMMAND_JUDGE_MODEL`) | `gemini-3.1-flash-lite` | `none` |
| Read decision lead | `gemini-3.8-flash`, `gpt-6-luna` | `none` |
| Wake decision | lead `gemini-3.8-flash`, `gpt-6.1-sol`, then default `gpt-6-luna` (`WAKE_DECISION_MODEL`) | `none` |
| Memory extraction lead | `gpt-6-luna` | `low` |
| Aux fallbacks (`AUX_FALLBACKS`) | `gpt-6-luna`, `gemini-3.1-flash-lite` | task tier |
| Anthropic web-search fallback | `claude-haiku-5-5` | `low` |
| Vertex grounded search (`GROUNDING_MODEL`) | `gemini-3.1-flash-lite` | its default |
| OpenAI web-search fallback | `gpt-6.1-sol` | `none` |
| Claude Code (`model_claude_code`) | `claude-opus-5-5`, unchanged | `medium` |

The compactor, the conversation summary, OpenAI Decisions, image, voice and
the fix thread keep their defaults. Codex stays unset.

These models retire, each to the successor beside it:

| Retired | Successor |
|---|---|
| `claude-opus-5`, `claude-opus-5[1m]` | `claude-opus-5-5`, `claude-opus-5-5[1m]` |
| `claude-sonnet-5`, `claude-sonnet-5[1m]` | `claude-sonnet-5-5`, `claude-sonnet-5-5[1m]` |
| `claude-fable-5`, `claude-fable-5[1m]` | `claude-fable-5-1`, `claude-fable-5-1[1m]` |
| `claude-haiku-4-5` | `claude-haiku-5-5` |
| `gemini-3-flash-preview` | `gemini-3.8-flash` |
| `gpt-5.4-mini`, `gpt-5.6-luna` | `gpt-6-luna` |
| `gpt-5.6-sol`, `gpt-5.6-terra` | `gpt-6.1-sol` |
| `gpt-5.5-pro` | `gpt-6-astra` |

Disabled rows that had no successor (Opus 4.x, Sonnet 4.6, GPT-5.5, GPT-5.4)
get one too. A new test, `every_default_names_a_current_model`, fails if any
default, lead or fallback names a retired or missing row.

## Rationale

### Chat and background

- **Sonnet 5.5 is the chat default.** It scores 56.0 on the Artificial
  Analysis index against Opus 5.5's 57.6, at half the price. `high` is
  Anthropic's documented default for it. The maintainer wanted cost-conscious
  defaults with enough intelligence for common workspace tasks and triggers.
- **Haiku 5.5 is the background default.** It has the highest small-model
  index (43.4), the lowest price ($0.10 / $0.50), vision, and the widest
  reach: Vertex, Anthropic, the Claude gateway and OpenRouter. Thinking on
  costs about 14 s to first output, so these one-line tasks keep `none`.
- **The captures blamed effort, not models.** A model off a task's list runs
  at its own default effort (ADR 0390). That is how titles took 4 to 7 s.

### Eval-gated purposes

The three shipped evals gate their purposes' models. Every candidate ran at
`none`, for about $0.35 in total.

| Eval | Model | Result | p50 ms |
|---|---|---|---|
| `command-judge` | GPT-6 Luna | 0/24 false safe, 1/17 over-cautious. Pass. | 1409 |
| `command-judge` | Gemini 3.1 Flash-Lite | 0/24 false safe, 0/17 over-cautious. Pass. | 687 |
| `command-judge` | Gemini 3.8 Flash | 0/24 false safe, 8/17 over-cautious. Pass. | 4144 |
| `read-decision` | GPT-6 Luna | 0/29 missed, 6/47 noise passed. Pass. | 969 |
| `wake-decision` | GPT-6 Luna, 3 runs | 0/27 missed each run, noise 20, 21, 19 of 24. Pass. | 954-1511 |
| `wake-decision` | Gemini 3.1 Flash-Lite | 1/27 missed, 9/24 noise. Fail. | 607 |

- **The judge defaults to Flash-Lite.** It waved no irreversible command
  through, which is ADR 0406's bar. It was also the least over-cautious and
  the fastest.
- **GPT-6 Luna replaces GPT-5.6 Luna** in the read decision and memory leads.
  It is the successor at half the price. Its wake noise matches GPT-5.6
  Luna's (20 to 21 in ADR 0448).
- **The wake list drops its tail.** ADR 0448 admits only measured models.
  Flash-Lite missed a must-wake, so it stays out. GPT-6 Luna becomes the
  catalog default, so the effective list is the three measured models.
- **Memory extraction stays at `low`.** Its predecessor lost coverage at
  `none` (ADR 0377).

### Search and fallbacks

- **`AUX_FALLBACKS` is cheapest first.** GPT-6 Luna covers OpenAI and
  OpenRouter. Flash-Lite covers a Vertex project with no Claude enabled.
- **The Anthropic search fallback keeps its request shape.** It already sends
  every thinking-by-default model at `low`, the shape its tests pin for
  Haiku 5.5.
- **Grounded search names its model once.** `GROUNDING_MODEL` replaces two
  copies of the retired id.
- **The OpenAI search fallback is GPT-6.1 Sol**, the current mid-tier. One
  report says GPT-6 Luna refuses `web_search`.

## Consequences

- **Haiku 5.5 is unmeasured for the command judge and the wake decision.**
  This machine's Vertex project serves no Haiku 5.5 in any region, and there
  was no Anthropic key. So it joins neither list.
- **An Anthropic-only install judges commands on its chat model.** No measured
  judge model is reachable there, so the judge falls back to the chat model.
  Before, Haiku 4.5 sat in the fallbacks, and ADR 0406 measured it calling 2
  of 24 irreversible commands safe. Measuring Haiku 5.5 is the way to close
  this.
- **An Anthropic-only install asks no model for the wake decision**, as ADR
  0448 already provides. The parent wakes with no call.
- **Existing workspaces follow the new defaults**, the chat model included.
  No migration pins Opus. A stored pick of a retired model moves to its
  successor. The `CHANGELOG.md` entry is the notice and names the setting
  that switches back.
- **A retirement leaves coding-agent settings alone.** Claude Code and Codex
  name models from their own lineups, which still offer ids retired here. So
  `retire_model` now skips `model_claude_code`, `model_codex`, their tiers,
  both intelligence ladders, coding-agent drafts, queued coding-agent spawns
  and coding-agent turns. This amends ADR 0418's list of rewritten settings.
- **A retirement never duplicates a pair.** Several models retire to one
  successor. A `model=value` list that already names the successor keeps the
  user's own entry and drops the retired one.
- **Old exchanges keep their labels.** `STATIC_MODEL_LABELS` in the frontend
  names each retired id.
- **The labelled sets are small and synthetic.** A clean score is evidence,
  not proof. Rerun the eval before any later change to these lists.

## Alternatives considered

**Haiku 5.5 as the judge, and as the wake tail.** It is the background
default and the cheapest capable model. Rejected because it could not be
measured, and ADRs 0406 and 0448 require a measured model for both purposes.

**Gemini 3.8 Flash as the judge.** It passed, but called 8 of 17 safe
commands unsafe, and its p50 was six times Flash-Lite's. Every false alarm is
a card the user must clear.

**Opus 5.5 as the chat default.** Slightly smarter, at twice the price. The
maintainer chose cost for the default. Opus stays one pick away.

**Sonnet 5.5 at `high` for Claude Code.** Rejected by the maintainer: Opus
5.5 at `medium` beats Opus 5 at max on coding, for less.

**Pin existing workspaces to Opus with a migration.** Rejected: a setting
nobody changed should follow the shipped default. The notice is enough.
