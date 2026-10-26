# 0575: Prompt cache TTLs come from evidence only: reported, else requested on an honoured route, else Unknown

- **Status**: Accepted
- **Date**: 2026-10-25
- **Amends**: [0479](0479-keep-alive-follows-the-observed-cache-ttl.md). An
  unknown TTL no longer beats as 5 minutes. It gets no beats.
- **Amends**: [0511](0511-subagent-waits-follow-the-observed-cache-ttl.md).
  Codex and an unknown TTL no longer state 5 minutes. The session context says
  the lifetime is unknown.
- **Amends**: [0556](0556-lucidos-agent-keep-alive-prefill-beats.md). The
  provider-keyed TTL table goes. A beat's TTL is the one its thread's latest
  cache write reported, which is the check decision 3 claimed.

## Context

An audit of every place the engine decides how long a prompt cache lives found
guesses. Code and docs:
`artifacts/investigations/prompt-cache-ttl-audit.md` in the development
workspace.

- An unknown TTL beat and read as 5 minutes (ADR 0479).
- Codex sessions were told 5 minutes whatever they got (ADR 0511). OpenAI
  documents 30 minutes for GPT-5.6 on, and Codex reports no lifetime at all.
- The Lucidos Agent keyed its TTL on the provider (ADR 0556). Nothing checked
  it against the usage block, although decision 3 said so.
- The switch estimate read an OpenAI 30-minute write as 5 minutes.
- OpenAI now documents a lifetime for models before GPT-5.6, which ADR 0556
  listed as undocumented.

The user's ask: use only actual TTLs, and never guess.

## Decision

A cache TTL comes from evidence, in this order:

1. **Reported.** The response splits the write by lifetime. That covers
   Anthropic, Vertex and the gateway (`ephemeral_5m/1h_input_tokens`), Claude
   Code's passed-through split, and OpenAI Responses
   (`cache_write_tokens_details.ttl_30m/12h_tokens`). The echoed
   `prompt_cache_options.ttl` names the whole write's lifetime when the split
   is absent.
2. **Requested.** The response splits nothing, and the request asked for a TTL
   on a route probes showed honours it. Today that is OpenRouter's
   `anthropic/` models, whose markers ask for 5 minutes. The provider reports
   what it asked, and the router keeps it only for OpenRouter.
3. **Unknown.** Anything else. A provider's documented default never counts.

`CacheTtl` holds the three, and every number lives on `CacheLifetime`, so
Unknown cannot reach one. A thread's TTL is that of its latest turn cache
write, for every agent.

Each consumer handles Unknown on its own terms:

| Consumer | Unknown does | Why |
|---|---|---|
| Session context | Says the lifetime is unknown, with a 4-minute cap | The cap fits the shortest lifetime a provider documents, so it is safe without claiming a lifetime |
| Keep-alive beat | Skips, reason `UnknownTtl` | A beat cannot be timed without a lifetime |
| Keep-alive status | `not_kept`, reason `unknown_ttl`, recorded | The user sees why nothing pings |
| Remembered request | Kept while the thread waits | Unknown never says the cache is cold. The waiting set bounds the store |
| Cache warmth | A third state beside warm and cold | Neither word would be true |
| Switch estimate | `rebuilds_cache: "unknown"`, with the price of a rewrite | The confirm says the switch *may* reread the history |
| Cache-warm line | `Not kept`, detail `Lifetime unknown` | The line still says nothing keeps the cache |
| Successor card and Continue | No `cold_at`, and the brief path | A narrative turn's cheap read cannot be claimed |
| Ended run | No `saved_usd` | A warm wake cannot be claimed |

Codex reports cache writes, never a TTL, so its captures now record the write
and its TTL stays Unknown. A 12-hour write prices at the new
`cache_write_12h` card rate, as a 1-hour write prices at `cache_write_1h`.

## Rationale

A guess is wrong somewhere, and wrong silently. The 5-minute default was safe
for the keep-alive's timing but told Codex the wrong cap, and it called OpenAI
caches cold 25 minutes early. A provider-keyed table goes stale the day a
provider or a setting changes, and the engine had no way to notice.

The response is the only witness of what the provider did with this request.
Where it says nothing, a request the route has been shown to honour is the
next best thing. Past that, saying "unknown" is honest, and each consumer can
choose the safe side for its own purpose.

A thread's latest turn write decides for every agent. Before, Claude Code read
the write, and the Lucidos Agent read a table, so the same thread could have
two TTLs.

## Consequences

- An OpenAI Lucidos Agent thread shows a 30-minute warm line and switch
  estimate, from its reported split.
- OpenRouter's non-Claude models, Chat Completions, xAI, Gemini and local
  servers read Unknown. They lose the 5-minute warm line, and a switch there
  asks with "may".
- Codex sessions are told the lifetime is unknown, and keep the 4-minute cap.
  Codex cost rises by the write premium it always paid.
- A thread whose writes report no lifetime gets no keep-alive beats.
- Old events carry only the 1-hour split. They read as before, since an absent
  key decodes as no split.

## Alternatives considered

- **State OpenAI's documented 30-minute default for Codex.** Rejected by the
  user: a documented default is still not what the response reported, and
  Codex may run with other settings.
- **Keep 5 minutes as the unknown fallback.** Rejected: it is the guess this
  decision removes. It also called OpenAI caches cold too early.
- **Keep the provider table for the Lucidos Agent.** Rejected: it would give a
  thread a second TTL definition, and the response already says the answer.
- **Read Vertex's `x-vertex-ai-cache-creation-*` headers.** Not needed: they
  repeat the body split the engine already reads.
