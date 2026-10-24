# 0525: The Lucidos Agent gets its own intelligence ladder over shared rung estimates, and prices move to a JSON price table with a workspace override

- **Status**: Accepted
- **Amends**: 0500, 0518
- **Date**: 2026-10-19

## Context

Coding agents pick from an *intelligence ladder* (ADR 0500). Chat threads have
none: the model registry's `enabled` flag decides what the chat picker lists,
and nothing binds an agent's chat pick. The estimates also live in two menu
files, scored on one blend, so a chat model has no band at all.

Prices sit in a Rust table (`llm/model_prices.rs`). The Token Cost plugin keeps
its own copy in `artifacts/token-cost/pricing.json`, so the two drift.

## Decision

- **Shared estimates, own on-lists.** One file, `llm/rung_estimates.json`,
  holds the bands for every (model, effort) pair. Each *ladder surface* (chat,
  Claude Code, Codex) keeps its own on-list: a shipped frontier, or the owner's
  override. The menu files lose their `intelligence_ladder` blocks.
- **Two intelligence profiles.** Each pair carries a `general` band (AA
  Intelligence Index blended with Epoch ECI) and a `coding` band (AA Coding
  Agent Index). Chat ranks on general, coding agents on coding. A missing
  coding score takes the pair's general score plus its model's mean measured
  gap (coding minus general), flagged as estimated. A model with no measured
  pair has gap zero. Falling back to the bare general score mixed two scales
  within one model, so Opus 5.5 max read below xhigh.
- **Cost band is token band times price.** The file ships a token band per
  pair, tokens per task as a multiple of the reference rung. The engine
  multiplies it by the price of the route in force. Only a free route, one
  that bills nobody, reads "included". A route the organisation pays for,
  such as a Claude gateway subscription, still reads a price. A rung is model plus effort, never a route.
- **The chat ladder replaces `enabled`.** Its on-list is the only switch for
  what the chat picker lists. Settings > Models becomes the catalogue. On
  upgrade each enabled model's default-effort rung starts on, so nothing
  disappears. Agents are bound to it as on the coding surfaces, under
  `intelligence_ladder_chat`.
- **Prices ship as a JSON price table.** `llm/model_prices.json` holds dated
  cards in Token Cost's format, each with a `source`. A workspace override in
  `data/config/model-prices.json` merges card by card. A card applies from its
  `from` day, so history keeps its prices. Every price reader goes through it,
  and Token Cost reads it instead of keeping its own.
- **Vocabulary.** *Intelligence rung* names a ladder step, *cache rung* a
  memory view's cache mark. No bare `rung` identifier stays in either area,
  and a grep guard keeps it so.

### Amendments

- **0500:** the menu files no longer store the shipped ladder, and a cost band
  is no longer stored. The shipped frontier is computed per surface, on its
  profile. The coding ladders reorder by the coding profile, and nothing else
  changes until the owner acts.
- **0518, as superseded by 0527:** the intelligence band is now two bands,
  one per profile, each a 2-point step and never the raw score. On the coding
  profile a higher effort can read lower, where the Coding Agent Index
  measures it lower. A model's unmeasured efforts keep its measured gap, so
  its higher efforts still rise.
- **0526:** the Lucidos Agent's rung control lists the chat ladder's rungs,
  as a coding agent's does, and its ‹ › steps move along them. A rung with a choice
  of backend then opens the preferred-provider step.

## Rationale

One estimate table means a new model is one row, not one per surface. A chat
model and a coding agent are judged on different work, so one score ranks
them badly: hence two profiles. A token band times a live price stays right
when a price changes, where a stored cost band goes stale. And one price table
means one place a price can be wrong.

The ladder replaces `enabled` because two switches for "listed" can disagree,
and a model can then be on the ladder but missing from the picker.

## Consequences

- The coding-agent shipped on-lists changed once, when the coding profile
  reordered them. The general bands did not move. The changed on-rungs:
  - Claude Code turned on Sonnet 5.5 at medium, Opus 5.5 at low and Sonnet
    5.5 at max. A later coding gap fill also turned on Sonnet 5.5 at xhigh.
    It turned off Haiku 5.5 at max, which Haiku 5.5 at xhigh
    beats on coding for less. It turned off Opus 5.5 at max, which Sonnet 5.5
    at max beats on coding at the same cost band.
  - Codex turned off GPT-6.1 Sol at high and at max, and GPT-6 Astra at max.
    GPT-6.1 Sol at xhigh scores higher on coding than all three, and costs no
    more.
  - Opus 5.5 has one coding row, at max, below Sonnet 5.5 at max. The
    estimator ignores it, so Opus ranks on its general score at every effort,
    marked estimated, and max reads as xhigh (`docs/temporary-measures.md`).
- `models.enabled` stays as a column, read by nothing, until a later
  migration drops it. The upgrade snapshots the models a workspace had on.
  The engine writes them into the chat override at its next boot, since the
  rung order lives where SQL cannot read it. A new database has no events at
  migration time, so it keeps the shipped chat ladder.
- A model the shipped pairs never name, a user's own included, lists on the
  chat ladder off and unbanded, at the rung it joins at: its default effort,
  else the tier nearest `medium` its first route takes.
- Retiring a model moves its chat ladder rungs to the successor, each at a
  tier it takes, and drops a rung the ladder already holds.
- Token Cost needs a one-time migration of its cards into the override.
- The card format gains an optional `cache_write_1h`, because the keep-alive
  and gateway telemetry price both cache TTLs. Absent, it equals
  `cache_write`.
- A dated card replaces a hand-timed edit: Gemini 3.8 Flash's 2027 price sits
  on a card from that day.
- The eval harness keeps its own pinned `prices.toml`. A published eval
  result must not reprice when the table changes, so it is a different
  concept, not a second copy.
- Every estimated model needs a price card, since its cost is computed. The
  table gained cards for the GPT-6 and GPT-5.6 families, Gemini 3.1 Pro
  preview and Flash-Lite, and Grok 4.6. Their output prices match what the
  old cost bands implied.
- Only chat knows its route in force. The coding-agent ladders price at list,
  because the engine does not know which backend Claude Code or Codex calls.
  A subscription would still price at list: someone pays for it.
- An agent's pick is bound on chat wherever it names one: `run_thread`, a
  trigger's model, and a chat thread an agent starts over HTTP.
- Bands stay estimates. Nothing in Lucidos measures them.

## Alternatives considered

- **One shared on-list for every surface.** Simpler, but Codex cannot run a
  Gemini model, and a pair worth picking for chat may be a poor coding pick.
  Rejected.
- **Keep `enabled` beside the chat ladder.** Two switches for one question.
  Rejected.
- **Ship AA's cost per task as the cost band.** Republishes AA's figure (ADR
  0500) and ignores free routes. Rejected.
- **Fetch prices or estimates from a hosted endpoint.** A new runtime
  dependency and a privacy question, for data that changes monthly. Rejected.
- **Keep prices in Rust.** The owner cannot correct a price without a release,
  and Token Cost keeps its copy. Rejected.
