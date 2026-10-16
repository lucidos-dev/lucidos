# 0492: The 5-minute keep-alive beat fires at 230 to 260 s so its request lands near 4.5 minutes, amending the beat window of 0479

- **Status**: Accepted
- **Date**: 2026-10-16
- **Amends**: [0479](0479-keep-alive-follows-the-observed-cache-ttl.md) and
  [0470](0470-keep-the-prompt-cache-warm.md). Both fired the 5-minute beat
  from 200 s, about every 4 minutes.

## Context

The beat window counts seconds since the last request started. That is right:
Anthropic's 5-minute lifetime runs from the start of a request. Timing from
its end would make a long call lose the cache.

A beat restarts Claude Code cold. The cold start before the request has a
median of 16 s and a p95 of 30 s. The engine ticks every 10 s.

The dev workspace holds 32 real beats from the last 7 days. A beat at 200 s
landed at a median of 221 s (range 210 to 269 s). All 32 hit the cache.
Across all requests near the 5-minute mark, the cache holds until 300 s. It
is gone by about 320 s. So the beat landed about 80 s earlier than needed.

## Decision

The 5-minute window is 230 to 260 s. The 1-hour window stays 3300 to 3360 s.
An unknown TTL still beats as 5 minutes.

## Rationale

- **The request lands near 4.5 minutes.** A beat at 230 s lands at about
  251 s, and at about 270 s at p95. That leaves 30 s of margin before 300 s.
- **About 12% fewer beats.** Each beat is a full-context read plus a turn.
- **The give-up point moves with it.** The budget is beats times the window
  start (ADR 0489). On Opus 5.5 a 5-minute session now gives up after
  25 x 230 s, up from 25 x 200 s. It still pays the same number of beats.

## Consequences

- **A 5-minute session beats about every 4 minutes.** That wording in
  ADR 0470 still holds. The request itself lands near 4.5 minutes.
- **The margin is 30 s at p95.** A slower cold start costs one rewrite. The
  window start is the one knob that widens it.

## Alternatives considered

- **Fire at 270 s.** Rejected: its p95 request lands past 300 s. A late beat
  costs a full rewrite (25 beats on Opus 5.5).
- **Time the beat from the end of the last request.** Rejected: the lifetime
  runs from the start, so a long call would lose the cache.
- **Keep 200 s.** Rejected: all 32 measured beats hit the cache with room to
  spare, so the early start only bought extra beats.
