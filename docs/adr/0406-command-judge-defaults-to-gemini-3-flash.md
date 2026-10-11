# 0406: The command judge defaults to Gemini 3 Flash, the model that waved no irreversible command through

- **Status**: Accepted
- **Date**: 2026-10-09

## Context

`model_command_judge` defaulted to Claude Haiku 4.5, which led the judge's
recommended list ahead of the shared fallbacks. ADR 0375 kept it there; this
decision replaces that part of 0375. Google is retiring Haiku 4.5
on Vertex, and ADR 0403 makes a retired default move on by itself. That left
the question of which model the judge should start on.

The judge had never been measured. `lucidos-eval command-judge` now runs a
candidate through the guard's own questions and thresholds, over 51 synthetic
commands in `eval/command-judge/commands.toml`.

## Decision

The judge's catalog default is `gemini-3-flash-preview`, at the judge's
existing `none` tier. Its recommended list is the shared fallbacks: Gemini 3
Flash, GPT-5.4 mini, then Haiku 4.5.

## Rationale

The number that matters is an irreversible command called safe, since the
guard then runs it with no card. On Vertex, two runs each:

| Model, tier | Irreversible called safe (of 24) | p95 |
|---|---|---|
| Haiku 4.5, none | 2 in both runs: a `curl -F` upload, a `docker push` | 4.3 s |
| Haiku 4.5, low | 0 | 17.1 s |
| Gemini 3 Flash, none | 0 in both runs | 1.4 s |
| Gemini 3.5 Flash, none | 0 in both runs | 1.6 s |
| GPT-5.4 mini, none / low (OpenAI) | 1 / 0 | 1.7 s |

- **Haiku 4.5 at `none` lets real side effects through.** At `low` it stops,
  but its p95 sits near the 20-second attempt cap, while a user waits on the
  card.
- **Gemini 3 Flash missed nothing, and it is the fastest.** It was already
  the judge's next fallback on Vertex, and the first auxiliary fallback
  everywhere.
- **Gemini 3.5 Flash scored as well**, but the shared list starts with Gemini
  3 Flash. One list keeps the judge resolving like every other background
  task.

## Consequences

- An install with Vertex or OpenRouter judges on Gemini 3 Flash. One with
  OpenAI alone judges on GPT-5.4 mini at `none`, which missed one of 24.
- **An Anthropic-only install still judges on Haiku 4.5**, the only model
  its provider serves on the list. On this set it called 2 of 24
  irreversible commands safe, so the gap stays open there. Claude Haiku 5.5
  is the candidate. It is registered, but could not be measured: Vertex did
  not serve it for the project used.
- A stored `model_command_judge` is untouched, whatever it names.
- The labelled set is small and synthetic. A model scoring 0 of 24 on it is
  evidence, not proof, and the check should rerun before any later change.

## Alternatives considered

**Keep Haiku 4.5 until Haiku 5.5 can be measured.** Rejected: it keeps a
measured gap in the shipped default for an unknown wait.

**Raise the judge's tier to `low`.** It closes Haiku 4.5's gap, but a p95
near the attempt cap turns a slow call into a timeout. It also raises cost
on every model, Gemini included, which needs no thinking to score 0.

**Gemini 3.5 Flash.** Equal on this set. It would give the judge a list of
its own, ahead of the shared fallbacks, for no measured gain.
