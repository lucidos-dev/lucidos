# 0581: The keep-alive requires a readable cache, and savings require a wake that held the context

- **Status**: Accepted
- **Date**: 2026-10-26
- **Amends**: [0489](0489-keep-alive-budget-follows-model-prices.md),
  [0556](0556-lucidos-agent-keep-alive-prefill-beats.md) and
  [0563](0563-cache-keep-alive-speaks-one-grammar.md).
- **Relates to**: 0470, 0492, 0547 and 0575.
- **Amended by**: [0580](0580-keep-alive-missed-ping-is-thread-scoped.md).
  Decision 5's provider guard no longer pauses a provider for a missed beat.
  Three misses in a row pause it for an hour.

## Context

A week of Token Cost data appeared to show little benefit from keep-alive
pings. Rebuilding the runs from events exposed two measurement errors.
The rollup priced reads without writes, and treated old unscored runs as
runs that saved nothing. The rebuilt week saved $61.84 against $23.30 of
pings, across 146 runs.

Thirteen missed pings cost $8.28 by rewriting the context. Five resume beats
followed a switch from Sonnet to Opus. Every unchanged-model resume held,
306 of 306, and every session-copy beat held, 210 of 210. Eight Lucidos
Agent prefill beats missed on their first ping. None missed because of timing.

The proposed prefill cause was a cut with no matching cache breakpoint.
Request tests show that the cut already preserves an explicit marker from
the turn, with the same content through it. It omits a view block, so its
fingerprint can differ from the turn's stable prefix. That does not prove
why the provider missed the earlier marker.

The same content did not mean the same bytes. The cut went through a
`serde_json::Value`, which writes its keys sorted, while the turn sends its
struct's key order. See § Probe result.

## Decision

1. **A beat requires the model and effort that wrote the cache.**
   Compare exact model ids, including a context-window suffix such as `[1m]`.
   A mismatch reads `not_kept: model_changed`. Resume sends pin the checked
   settings; copy and prefill sends check their final parameters again.
2. **Prefill beats go out with the turn's own bytes.**
   The cut clones the turn's request type and writes it with the same
   serialization, so key order matches the turn. The gateway keys the cache
   on raw bytes, and the old cut wrote sorted keys. The turn's bytes held 10
   of 10 first beats, sorted keys 3 of 10 (Fisher p 0.003). See § Probe result.
   Prefill captures record the fingerprint of the prefix their cut sends.
   The earlier pause is gone, and ADR 0580's provider guard stays the backstop.
3. **A request held the context when it read at least half its input from cache.**
   Empty input does not count. One divisor supplies the Rust predicate and
   its SQL expression. Each API call is judged from its final capture frame.
4. **A missed beat ends its run until a real request starts.**
   The status reads `not_kept: beat_missed`, derived from persisted captures.
   It survives a restart, forgetting a prefill pair, and a later held beat.
   The engine logs the thread, route, model, read and input on a miss.
5. **The prefill write-share judge stays.**
   A prefill that writes over 2% ends its run, with the existing first-beat
   provider guard. Resume and copy beats use only the held-context predicate.
6. **A saving requires a warm wake that held the context.**
   Reading the shared system prompt alone earns no saving. Known TTL
   evidence and the gap from the last refresh still bound warmth.
7. **The break-even budget and beat windows stay.**
   No ping cap, state-specific wake rate or hazard rule joins them.
   Historical events stay unchanged. Token Cost labels old unscored runs
   and counts missed pings separately.

## Probe result

A temporary probe settled the prefill miss on the Claude gateway. Plan:
`docs/plans/2026-10-26-prefill-beat-byte-order-probe.md`. Each pair sent a
synthetic turn of about 13.6k tokens with a unique prefix. After 260 s, it
sent the turn's whole cut as a beat, then again 10 s later. The two arms
differed only in JSON key order.

| Arm | First beats that held | First beat on a miss | Second beat |
|---|---|---|---|
| Turn's own bytes | 10 of 10 | none | read 13,637 in 10 of 10 |
| Sorted keys (the old cut) | 3 of 10 | read 0, wrote 13,637 | read 13,637 in 10 of 10 |

Every turn wrote 13,637 tokens and read none. A Fisher exact test gives a
two-sided p of 0.003. The sorted beats that missed wrote a cache entry of
their own, which their second beat then read. So the gateway keys on the raw
request bytes, not only on the rendered prompt.

The fix sends a beat the turn's own serialized bytes, key order included.
Its regression tests pin the bytes of both cuts. With the cause fixed,
prefill beats are on again, as Decision 2 states.

## Rationale

A missed ping can rewrite the entire context at 25 times the read price.
Preventing that miss comes before capping the damage afterward.
The model guard and the byte-exact prefill cut cover every observed miss in the week.
The backstop handles an unexpected miss without paying it again in that run.

Observed reads were bimodal: warm requests read 85% to 99%, and misses read
0% to 15%. Half lies between them. It judges reported usage and assumes
no undocumented lifetime.

Caps from 2 to 12 pings removed none of the misses and usually reduced net
savings. A miss can happen on the first ping. Break-even already prices
long waits without predicting when they end.

## Consequences

- Lucidos Agent threads receive prefill beats again.
  `prefill_paused` is historical: stored events still carry it, and the
  engine never emits it.
- A failed ping can pause its provider until restart, reported as `provider_paused`.
  Only a request without a prefill pair or backend reports `uncacheable_provider`.
- Claude Code resume and copy beats continue when their model and effort match.
- A provider can still evict an entry early. The backstop permits at most
  one missed beat before the next real request.
- Prefill fingerprints expose a cut mismatch without persisting request bodies.
  Claude Code captures remain without a fingerprint, as their turn captures do.
- A past run without scoring evidence remains unscored, rather than earning
  a fabricated zero saving. Pings are priced at their full reported usage.

## Alternatives considered

- **Cap pings or fit a wake rate for each state.** These left the costly
  first misses intact and lost useful warm wakes.
- **Only stop after a miss.** This still pays every first miss.
- **Move every resume to a session copy.** Copies take the current model too,
  so they need the same guard. The guard covers the observed resume misses.
- **Change the prefill cut without proving the cause.** A different marked
  prefix alone does not prove the provider will read the entry.
- **Persist a provider ban across restarts.** A beat defect does not prove
  every thread on that provider cannot prefill.
- **Apply the 2% write-share judge to every route.** A copy writes about 1.8%
  by design. Its run needs only to hold the context.
