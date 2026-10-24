# 0377: Memory extraction and the conversation summary lead with their measured models; no memory key inherits another; an empty extraction is final; Tree classification writes no queries

- **Status**: Accepted
- **Date**: 2026-10-06
- **Extends**: [0375: Every auxiliary model call runs through the router](0375-auxiliary-calls-run-through-the-router.md)

## Context

ADR 0375 gave every auxiliary purpose one *auxiliary default*: its catalog
default, then Gemini 3 Flash, GPT-5.4 mini and Haiku 4.5, else the chat model.
For the memory tasks that list was a reach fix, not a quality choice. Three of
their keys also inherited `model_memory`.

A bounded comparison measured seven models on each memory task's production
prompt, with two blind judges, cost and latency
(`docs/plans/2026-10-06-memory-tasks-model-comparison.md`). The tasks want
different things. Classification is on the user's wait before every turn.
Extraction runs on every event, so cost and rule-breaking facts dominate. The
summary runs rarely, nobody waits for it, and its errors persist.

## Decision

A purpose may lead its auxiliary list with the models its comparison measured
best, ahead of its catalog default. Two do:

| Purpose | Lead | Effort default |
|---|---|---|
| Memory extraction | GPT-5.6 Luna | `low` |
| Conversation summary | the compactor's list: GPT-6.1 Sol, Gemini 3.8 Flash, Sonnet 5.5 | `low` |

Classification and `find` keep ADR 0375's list at `none`. No memory key
inherits another, model or effort. `LUCIDOS_EXTRACTION_MODEL` still goes first.

Two related changes ship with it. An extraction reply of `[]` is final. Under
the Tree memory module, classification writes no search queries.

## Rationale

- **Each lead follows its task's evidence.** GPT-5.6 Luna at `low` kept
  extraction coverage within noise (0.87 against 0.91) with 0.87 fewer
  rule-breaking facts per item, at under half the effective cost. On summaries
  Gemini 3 Flash covered 0.73 and invented 1.27 claims per paragraph; GPT-6.1
  Sol covered 0.99 and invented 0.03, Sonnet 5.5 0.98 and 0.33.
- **A lead, not a second scheme.** The lead is a prefix on ADR 0375's list, so
  one resolver, one recommendation route and one fallback chain still hold.
  The summary reads `COMPACTOR_DEFAULTS`, so the two summary writers share one
  definition.
- **`low` for every extraction model.** Luna lost 15 points of coverage at
  `none`, and GPT-5.4 mini 14. Gemini 3 Flash measured the same at `low` as at
  `none`, at 28% more cost. One effort per purpose keeps ADR 0375's selection
  shape.
- **No inheritance.** A stored `model_memory` was chosen for extraction. Gemini
  3.5 Flash, the value the dev workspace stored, wrote the worst search queries
  and added nothing on summaries.
- **`[]` is an answer.** Retrying it shopped for a different one, and three of
  them stored the raw message as a fact. That was 4.7% of the dev workspace's
  recent memory, and it ruled out any model that says `[]` often.
- **Tree reads no recall**, so the second classification call cost the user
  about 1.2 s on most turns for queries nothing read.

## Consequences

- An install that serves Luna extracts on it; one that serves Sol, Gemini 3.8
  Flash or Sonnet 5.5 summarises on it. Settings recommends the leads first.
- A workspace that stored `model_memory` keeps it for extraction only.
  Classification, the summary and `find` move to their own defaults.
- Extraction on Gemini 3 Flash costs about 28% more, at `low`.
- OpenRouter gains a route to GPT-5.6 Luna, so an OpenRouter-only install
  reaches the lead.
- A failed call or an unreadable reply still retries and falls back to the raw
  text. Fallback facts already stored stay until a memory rebuild.
- `lucidos-eval` pins the memory keys to Gemini 3 Flash at their earlier
  efforts, so its runs stay comparable whatever the host serves.

## Alternatives considered

- **Per-entry tiers on the list.** The backup design ran Gemini 3 Flash at
  `none` behind Luna at `low`. Lost: it needs a tier per list entry in ADR
  0375's selection, for a 28% cost difference on one fallback.
- **Make Luna the catalog default.** It resolves the same, since Gemini 3 Flash
  heads the fallbacks anyway. Not taken: the eval pins and prices the catalog
  default, and a lead keeps that the model a Vertex install runs.
- **Keep inheritance from a stored `model_memory`.** Offered and declined: it
  would leave the classifier and summariser on a model chosen for extraction.
- **Gemini 3.8 Flash for classification.** Lost: it has no `none` tier, and at
  `low` one turn took 53 s, past the 30 s deadline.
- **GPT-6 Luna, the cheapest.** Lost: it returned `[]` on 83% of extraction
  events and missed memory on two thirds of the turns that needed it.
- **Haiku 4.5 higher, or for summaries.** Lost: it broke the extraction JSON on
  7 of 30 items, and covered 0.30 of a summary with 3.45 invented claims.
- **The backup's own routing module.** Superseded by ADR 0375 before it
  applied. Its leads and fixes are what this ADR re-applies.
