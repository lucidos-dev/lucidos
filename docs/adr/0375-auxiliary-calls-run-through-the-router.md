# 0375: Every auxiliary model call runs through the router, with a provider-aware default and its attempt cap on each request

- **Status**: Accepted
- **Date**: 2026-10-06
- **Extended by**: [0377: Memory extraction and the conversation summary lead with their measured models](0377-memory-tasks-lead-with-their-measured-models.md)

## Context

Titles, change summaries, image descriptions, fact extraction, query
classification, conversation summaries, `find` and the command judge built
their provider through `MemoryExtractor::provider_for_model`. It sent `gpt-*`
to OpenAI and everything else to Vertex. The extractor existed only when a
Vertex project was configured, and was built once at boot.

So an install on Anthropic, OpenAI or OpenRouter alone got no titles, no fact
extraction and no change summaries. Almost every site skipped silently. The
Tree compactor had the same bug, fixed in ADR 0373 by calling through the
router. The evidence and the per-install table are in
`docs/plans/2026-10-06-auxiliary-calls-through-the-router.md`.

## Decision

Every auxiliary model call resolves its model per call against the engine's
live router, and runs through it. An unset model takes the *auxiliary
default*: the purpose's catalog default, then Gemini 3 Flash, GPT-5.4 mini and
Haiku 4.5, the first a configured provider serves, else the chat model. The
purpose's attempt cap rides on each request, never on a client.

## Rationale

- **The router already reaches every backend.** A second routing table in the
  extractor drifted from it, and is what stranded every non-Vertex install.
- **The default extends what installs already ran.** The catalog default stays
  first, so a Vertex install keeps Gemini 3 Flash and the judge keeps Haiku 4.5.
  The fallbacks are the cheapest models Settings already offered, and between
  them they reach Vertex, OpenAI, Anthropic and OpenRouter.
- **The chat model is the last resort.** It covers any install that can chat
  at all, such as local-only or xAI-only. The router clamps the purpose's
  effort to the lowest tier that model offers.
- **A stored pick is honoured or refused, never substituted.** A pinned model
  may be a data-region choice, so moving its content to another vendor would be
  data exposure. Settings shows the refusal on the row instead.
- **The cap belongs to the request.** The router's backends are shared with
  turns, which must never be capped, and their clients carry long timeouts.
  `RequestBuilder::timeout` overrides the client per request, so ADR 0107's
  budgets hold on every backend, Anthropic included.
- **One list, two readers.** The list a purpose resolves from is what Settings
  recommends first, served by `GET /api/v1/models/background`.

## Consequences

- `MemoryExtractor` the struct is gone, with its `Option` gate. Its prompts and
  parsing stay as functions in `memory/extractor.rs`.
- A credential added at runtime moves the next call, with no restart.
- Haiku 4.5 and GPT-5.4 mini are builtin registry rows with first-party and
  OpenRouter routes, and Gemini 3 Flash gained an OpenRouter route. All three
  are chat-model choices too.
- Every background picker offers every model the chat picker offers, with the
  recommended ones in a section first. Background rows show no provider step,
  since a background task stores no backend.
- Installs that ran no background calls now pay for them on the provider they
  have.
- A mock engine runs these calls on its mock, so e2e spends nothing on them.
- Image description could be pointed at a model that cannot read images, and
  every call then failed with only a log line. ADR 0379 closes this with a
  vision flag on the model registry.

## Alternatives considered

- **Keep the extractor and add providers to it.** Lost for the reason ADR 0373
  gives: a second routing table drifts from the router.
- **One fixed fallback model.** Lost on reach: no single model is served by
  every provider an install might hold.
- **Fall back from an unreachable stored pick to the default.** Lost: it moves
  the user's content to a vendor they did not pick, silently.
- **Rebuild the router's backends per call with a short client timeout.**
  Lost: it builds HTTP clients on every call, and the request-level timeout
  already does the job.
- **Prefer the provider the user chats with.** Not adopted: background calls
  already went to Vertex for users chatting elsewhere, and the list order is
  simpler to reason about.
- **Keep a curated background picker.** Declined by the user: every picker
  offers every model, and the recommendation is a section, not a filter.
