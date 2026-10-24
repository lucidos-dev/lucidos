# 0373: The Tree compactor's default is resolved against the configured providers, from one ordered list

- **Status**: Accepted
- **Date**: 2026-10-06

## Context

The compactor writes every summary line of the Tree memory module (ADR 0362).
A bad line costs every later turn that reads it. Its default model was
inherited from the memory model, `gemini-3-flash-preview`. Its calls went
through the background-task extractor, which exists only when Vertex is
configured. So on a workspace without Vertex the compactor failed every node,
whatever model was picked.

Users configure different providers: Vertex, Anthropic, OpenAI, OpenRouter, or
a local server. No single model is reachable on all of them. A bounded
comparison measured each candidate's summaries for coverage, invented claims
and lost user words, with cost and time. The evidence and method are in
`docs/plans/2026-10-06-tree-compactor-provider-aware-default.md`.

## Decision

The compactor's default is the first of GPT-6.1 Sol, Gemini 3.8 Flash and
Sonnet 5.5 that a configured provider serves, at `low`. It applies while
`model_summary_compaction` is unset. With none reachable, the compactor runs on
the workspace's chat model. It calls through the engine's router, so any
configured provider serves it.

## Rationale

- **Reach needs a list, not a model.** Each family reaches different
  providers. A list resolved against the router's configured set gives each
  install the best model it can reach. The chat-model fallback covers any
  install that can chat at all.
- **The order is quality first, within reason on cost.** GPT-6.1 Sol covered
  the most and lost almost no user words. Gemini 3.8 Flash ranks above Sonnet
  5.5: it is a quarter of the price, already far more faithful than the old
  default, and carries no Anthropic spend-cap risk.
- **`low` everywhere.** `medium` bought nothing measurable on Gemini 3.8 Flash
  or Sonnet 5.5, and GPT-6.1 Sol has no lower tier.
- **The picker offers one model per family**, each the best measured on its
  routes. A longer list offered models that lost on every axis.
- **One source.** The list, the prices and the per-model token seeds live in
  `summary_tree/compactor_models.rs`. The estimate, the picker and the default
  all read it.

## Consequences

- A workspace that set only `model_memory` or `model_conversation_summary` no
  longer has the compactor follow it. The compactor's keys are optional
  preferences with no catalog default, and Settings reads the resolved choice
  from `GET /api/v1/memory/tree-compactor`.
- The compactor no longer gets a per-attempt HTTP timeout. Its 85-second node
  deadline still bounds every node, like the agent-model purposes.
- GPT-6.1 Sol and Gemini 3.8 Flash are builtin registry rows with an OpenRouter
  route, so they are chat-model choices too. Neither is offered `none`, which
  both reject.
- The default is reconsidered when a new model family ships. A new candidate
  goes through the same comparison before it joins the list.

## Alternatives considered

- **One fixed default model.** Lost on reach: every fixed choice leaves at
  least two provider setups unable to compact.
- **Sonnet 5.5 above Gemini 3.8 Flash, for quality.** Lost: four times the
  cost, for less coverage than GPT-6.1 Sol buys at half of that.
- **Gemini 3.8 Flash first, for cost.** Offered and declined: GPT-6.1 Sol's
  gain in coverage and in kept user words is worth its 2.4 times the cost.
- **Keep inheriting the memory model.** Lost: that model is tuned for tiny
  per-turn calls at `none`, and the default it gave invented tool output.
- **Keep the extractor path and add providers to it.** Lost: the router already
  reaches every provider and clamps tiers per route. A second routing table
  would drift from it.
- **Cheaper candidates.** Gemini 3.5 Flash-Lite lost a quarter of the coverage.
  GPT-6 Luna and GPT-5.6 Luna invented as much as the old default. Haiku 4.5
  was worse on both axes and overran the size limit.

## Amendment, 2026-10-06: the cap returns and the picker opens

ADR 0375 moves every auxiliary call onto the router with its attempt cap on
each request, so the compactor gets its per-attempt cap back. Its default list
now resolves through the shared `aux_purpose::select`. The picker offers every
model, with this list in a Recommended section first, by the user's decision.
The estimate prices only the measured models. `GET /api/v1/models/background`
replaces `GET /api/v1/memory/tree-compactor`.
