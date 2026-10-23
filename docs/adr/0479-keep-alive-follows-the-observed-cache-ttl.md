# 0479: The cache keep-alive times its beats from the TTL each Claude Code session shows; unknown beats as 5 minutes

- **Status**: Accepted
- **Date**: 2026-10-14
- **Amends**: [0470](0470-keep-the-prompt-cache-warm.md). Its keep-alive
  assumed every Claude Code session has a 5-minute cache.
- **Amended by**: [0489](0489-keep-alive-budget-follows-model-prices.md). The
  150-minute cut-off below gave way to a budget from the model's prices.
- **Amended by**: [0492](0492-keep-alive-beat-lands-near-4-5-minutes.md). The
  5-minute window now starts at 230 s, not 200 s.
- **Amended by**: [0556](0556-lucidos-agent-keep-alive-prefill-beats.md). The
  engine's own requests state their TTL, and OpenAI adds a 30-minute one.

## Context

ADR 0470's keep-alive beat every Claude Code session about every 4 minutes.
That matches a 5-minute cache only. Claude Code's TTL depends on how the user
is billed:

| Billing | Default TTL | Override |
|---|---|---|
| API key, Vertex, Bedrock, Foundry | 5 minutes | `ENABLE_PROMPT_CACHING_1H` |
| Pro or Max subscription | 1 hour | `FORCE_PROMPT_CACHING_5M` |

Settings `promptCacheTtl` and `subagentPromptCacheTtl` can change it too. So a
subscription session got a beat about every 4 minutes although its cache lived
an hour. Each beat is a full-context cache read plus a turn, charged to the
user's usage limit, and a wait under about 55 minutes needs none.

## Decision

The keep-alive reads the TTL each session actually shows, and beats just
before it would expire:

| Observed TTL | Beat window, seconds since the last request started |
|---|---|
| 5 minutes | 200 to 260 |
| 1 hour | 3300 to 3360 |
| Unknown | 200 to 260 |

The TTL is that of the session's latest cache write. Claude Code's usage block
splits each write by lifetime (`cache_creation.ephemeral_5m_input_tokens` and
`ephemeral_1h_input_tokens`). The engine records the 1-hour part on
`ContextCaptured` as `usage.cache_creation_1h_tokens`.

## Rationale

- **Observed, never guessed.** The provider, the billing, the environment and
  the settings all move the TTL. Only the usage block says what a request got.
- **The latest write decides.** A cache hit refreshes an entry at the lifetime
  it was written with, so a read-only request changes nothing. A sub-agent's
  write is skipped: `subagentPromptCacheTtl` can differ from the session's.
- **Any 5-minute token makes it 5 minutes.** That part expires first.
- **An unknown TTL beats as 5 minutes.** The two mistakes are not symmetric.
  Beating a 1-hour cache every 4 minutes wastes reads but keeps the cache.
  Beating a 5-minute cache an hour late loses it on every wait.
- **The 1-hour window leaves 4 to 5 minutes of margin.** A beat restarts the
  session cold, so its request can start tens of seconds late. One beat an
  hour costs the same wherever it sits, so the wider margin is free.
- **The 150-minute cut-off holds for both TTLs.** It bounds a stuck wait in
  wait time. Under a 1-hour TTL it allows at most two beats. Each costs a
  twentieth of the re-write it saves: a read is 0.1x base input, a 1-hour
  write 2x.
- **The keep-warm kinds hold too.** Under a 1-hour TTL most of their waits
  need no beat (background tasks: median 8 minutes, p99 60). The measured long
  tail outlasts the cut-off either way.

## Consequences

- **A 1-hour session gets one beat where it got about seventeen**, on a wait
  of about an hour. A wait under 55 minutes gets none.
- **A 5-minute session beats exactly as before.**
- **`ApiUsage` carries the 1-hour part of a write.** Only Claude Code captures
  fill it. A cost display can now price 1-hour writes at their own rate.
- **ADR 0470's rejection of the 1-hour TTL stands.** Lucidos still sets no TTL
  for Claude Code. It follows the one the session uses.
- **No 1-hour cut-off or keep-warm list was tuned.** ADR 0470's measurement
  covers 5-minute sessions only.

## Alternatives considered

- **Infer the TTL from the provider or the config dir.** Rejected: the
  environment variables and settings override both, and a Claude Code release
  can change a default. The changelog fixed one such fallback already.
- **Store a TTL enum on the capture instead of the 1-hour count.** Rejected:
  the count is what the provider reported, the TTL is a pure function of it,
  and pricing needs the count. Two stored forms would drift.
- **Treat an unknown TTL as 1 hour.** Rejected: on a 5-minute session it loses
  the cache on every wait, the exact re-write the keep-alive exists to prevent.

Plan and invariants: `docs/plans/2026-10-14-keep-alive-follows-the-cache-ttl.md`.
