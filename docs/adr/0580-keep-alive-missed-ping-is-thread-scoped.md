# 0580: A keep-alive ping that misses the cache ends only its own thread's run, and three misses in a row pause the provider for an hour

- **Status**: Accepted
- **Date**: 2026-10-26
- **Amends**: [0581](0581-keep-alive-never-pings-a-cache-it-cannot-read.md),
  decision 5. A prefill beat that misses no longer pauses its provider until
  a restart.
- **Amends**: [0556](0556-lucidos-agent-keep-alive-prefill-beats.md). The
  first-beat guard keeps only refused and talkative beats.
- **Amends**: [0563](0563-cache-keep-alive-speaks-one-grammar.md). The
  `not_kept` reasons gain `pings_paused` and `cache_expired`, and `restarted`
  narrows to its literal meaning.
- **Amended by**: [0582](0582-keep-alive-beat-keeps-its-request-and-give-up-cause.md).
  A beat acts only on the pair it sent, and give-up keeps its cause.

## Context

A Lucidos Agent thread on the Claude gateway showed no keep-alive card after
one ping. The first prefill beat since the engine started read 0 cached tokens
and wrote 79,024. The turn four minutes earlier had read 78,365. The
first-beat guard then paused the whole provider until a restart, so every
Lucidos Agent thread on it lost its pings.

The status then gave a false cause twice: the provider's pause, then
`restarted` once the store pruned the expired request, though the engine never
restarted. The run that beat ended also closed with 0 beats and no cost,
because the close copied the last stored `warming`.

ADR 0581 pauses prefill beats until a miss has a reproduced fix. It ends a
missed run with `beat_missed`, and it kept the first-beat guard for a writing
beat. This ADR settles what that guard does once prefill beats return.

That first beat's miss now has a cause. Its cut sent the turn's content with
its JSON keys sorted, and the gateway keys on raw bytes. A probe held 10 of
10 first beats with the turn's own bytes and 3 of 10 with sorted keys
(ADR 0581 § Probe result). The guard below still covers a miss with another
cause.

## Decision

1. **A prefill beat that misses ends that thread's run only.** A miss is a
   write over 2% of the input, or a beat that did not hold the context
   (ADR 0581 decision 3). The thread keeps a tombstone of its provider and
   reads `beat_missed`, ADR 0581's reason, until its next request.
2. **Repeated misses pause the provider, and the pause expires.** Three
   missed beats in a row on one provider, with no read between them, pause its
   beats for one hour (`MISSES_TO_PAUSE`, `PAUSE_AFTER_MISSES`). A read resets
   the streak. After the hour the next due beat tries again, and one more miss
   pauses it again at once. Its threads read `pings_paused`, with `until` on
   the status.
3. **A refused first beat, or a beat that writes output, keeps the verdict.**
   Both show the beat's shape is wrong for the provider. It stays paused until
   a restart and reads `provider_paused` (ADR 0581).
4. **A pair past its cache lifetime becomes a tombstone, never nothing.** Its
   thread reads `cache_expired`. `restarted` means Lucidos holds no record of
   the thread's last request: a fresh engine, a thread never seen, or an ended
   Claude Code session.
5. **A run's close counts its beats from its events.** The stored `warming`
   predates a beat that landed after the last tick, such as the one whose miss
   ended the run. The close keeps the larger count.

## Rationale

- **One miss says the beat missed this cache, not that the provider has
  none.** On the dev workspace 8 of 13 first beats in a run missed, and every
  later beat in a run hit. The turn after the incident's beat also read 0
  tokens, so the beat reached a different cache from the turns.
- **A streak still bounds the cost.** A provider whose beats always miss pays
  three rewrites, then one per hour. That is the same order as one probe,
  without a restart to recover.
- **A status that names the wrong cause is worse than none.** The user read
  that Lucidos restarted and looked for a restart that never happened.

## Consequences

- ADR 0581 paused prefill beats for a time, so the streak and the pause waited
  for them. Prefill beats now go out, and the streak and the pause apply.
- The memory store keeps tombstones while their thread waits, and drops them
  with the thread, as it drops pairs.
- `CacheKeepAliveStatus::NotKept` gains an optional `until`, set for
  `pings_paused` only.
- The verdicts stay in memory, so a restart still clears a pause.

## Alternatives considered

- **Keep the guard for a writing first beat.** One intermittent miss would
  stop every thread on the provider until a restart, as in the incident.
- **Expire the until-restart pause after a timeout.** One miss would still
  stop every thread for the timeout, though most of them would have hit.
- **Never pause, only end the thread's run.** A provider whose beats always
  miss would pay one rewrite per thread per wait, with no bound across
  threads.
- **Carry the pause end inside the reason.** A reason with a field changes the
  wire shape of every reason. An optional `until` beside it keeps every
  existing reader working.
