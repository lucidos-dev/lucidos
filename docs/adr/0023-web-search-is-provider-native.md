# 0023 — Web search is provider-native, resolved over the configured provider set

- **Status** — Accepted
- **Date** — 2026-07-27

## Context

`web_search` reached through `MemoryExtractor` into a `VertexProvider` and called
`search_with_grounding`. Two consequences, both realised:

1. **Non-Vertex users had no web search at all.** With no `VERTEX_PROJECT_ID` the
   extractor is `None`, and the tool answered
   `Error: web_search requires Vertex AI configuration`. Vertex was not the
   default search backend — it was the only one.
2. **A chat-region change silently broke search.** The grounding call routed
   through `endpoint_for_model`, which sends any non-`gemini-3*` model to the
   configured region. A workspace pinned to `vertex_region = eu` — the correct
   setting to reach Claude Opus 5 there — asked a multi-region that publishes no
   Google models for `gemini-2.5-flash-lite`, and every search 404'd from
   2026-07-03 onward.

Both trace to the same root: search was welded to one provider's private method
instead of sitting behind an abstraction.

We had already been burned once in the other direction. Commit `8b917c19b`
(2026-02-27) replaced a DuckDuckGo backend with Gemini grounding because
*"DuckDuckGo was serving CAPTCHAs, breaking all web searches"* — which is exactly
what created the Vertex coupling.

## Decision

Web search runs on a `WebSearchProvider` trait with one backend per
search-capable provider — Vertex (Gemini grounding), Anthropic (the `web_search`
server tool), OpenAI (Responses `web_search`) — selected by a `WebSearchChain`.

Three rules make it work:

1. **Resolve over the configured provider *set*, not the chat model's provider.**
   Search is a background capability like memory extraction, not a property of
   the model the user is chatting with. This *is* the fallback mechanism: a user
   on OpenRouter or a local endpoint — neither of which exposes a search tool —
   still searches through any other configured provider.
2. **Fixed preference order: Vertex → Anthropic → OpenAI.** Vertex first so
   existing workspaces see no behavior change. Anthropic before OpenAI because
   Anthropic's server tool has no per-call fee, while OpenAI's Responses
   `web_search` bills per call on top of the tokens the results consume. Not
   user-selectable; revisit only if asked for.
3. **Only availability failures fall through.** A backend returning a legitimate
   "no results" has answered — the chain stops there. Falling through on an empty
   result would re-run and re-bill every zero-result query against every
   configured provider.

Grounded search is additionally pinned to Vertex's **global** endpoint rather
than the configured region, because Google Search grounding is a global-endpoint
feature and the chat region may serve no Gemini models at all.

## Two standing *no*s

These are closed for different reasons. Neither should be re-proposed without new
evidence.

**Keyless scraping of a general engine** (DuckDuckGo, Google, SearXNG) — tried
and lost, see `8b917c19b`. General engines CAPTCHA unauthenticated datacenter
traffic by design, and DuckDuckGo's terms prohibit automated use (202 = soft rate
limit, 403 = IP flagged). It would fail the same way again. Note also that
DuckDuckGo has **no** official web-search API: its public Instant Answer endpoint
returns Wikipedia-style abstracts, not ranked results, and most queries return
nothing.

**A dedicated search vendor** (Brave, Tavily, Exa, Serper, Google CSE) — would
force a credential *and a credit card and a per-search bill* on every Lucidos
user for a capability their LLM provider already bundles. The category also
contracted sharply; surveyed 2026-07-27:

| Option | Status |
|---|---|
| Google Custom Search JSON | Closed to new customers; shuts down 2027-01-01 |
| Bing Search API | Retired by Microsoft in 2025 |
| Brave Search API | Free tier removed 2026-02; $5/1k, card required, attribution required |
| Serper | 2,500 free/mo, but scraped data — same fragility class as the 2026-02 failure |
| Tavily / Exa | $8 / $2.50 per 1k; neither operates its own index |

Meanwhile the LLM providers absorbed search into their own deals, which makes
provider-native the only tier that costs the user nothing extra.

## Consequences

- **Results are not uniform across users.** A user on Anthropic gets Anthropic's
  index; one on Vertex gets Google's. Accepted deliberately in exchange for not
  requiring a paid third-party signup to use a core tool.
- **A workspace whose only provider is OpenRouter or a local endpoint has no web
  search.** It gets one actionable error naming the three search-capable
  providers and where to add one — never a silent empty result.
- **The trait leaves the door open.** If the coverage gap above turns out to
  bite, a vendor backend is a drop-in addition to the chain rather than a
  redesign. That would be a new decision, not a reversal of this one.
- Adding a provider credential enables search without an engine restart: the
  chain is rebuilt by the same credential subscriber that swaps the LLM provider.

## See also

- `crates/lucidos-engine/src/llm/web_search/` — the trait, chain, and backends.
- `docs/plans/2026-07-27-web-search-provider-routing.md` — the implementation
  plan and its invariants.
