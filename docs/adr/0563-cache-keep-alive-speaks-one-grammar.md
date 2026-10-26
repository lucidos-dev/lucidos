# 0563: The cache keep-alive speaks one grammar

- **Status**: Accepted
- **Date**: 2026-10-23
- **Amends**: [0540](0540-a-keep-alive-run-leaves-one-card.md) ("a parked
  thread shows none") and
  [0547](0547-keep-alive-beats-a-parked-session-by-copy.md) (the cache-warm
  line as the only place a parked wait shows).

## Context

Three surfaces showed the *cache keep-alive*: the transcript card, the
*cache-warm line* pill and the waiting panel. They grew one at a time.

- **Coverage differed.** A wait parked on a question or permission card left no
  card and no record of its spend.
- **Wording differed.** One fact had three phrasings and three time words.
- **Engine types differed.** `CacheKeepAliveStatus` and `WarmLineKeepAlive`
  each had their own reason list.
- **A run's end said little.** The engine cleared the status, and the client
  inferred "Ended".

## Decision

One status type, one state set and one fact line serve every surface.

- **Every run leaves a card**, a parked wait included. A parked run's card sits
  inside the running turn, below the card it waits on, and opens no exchange.
- **One state set**: Warming, Stopped, Ended, Turned off. Not kept is a reason,
  not a state a card draws.
- **One engine type.** `CacheKeepAliveStatus` absorbs `WarmLineKeepAlive`, and
  `GET /threads/:id/cache-warmth` returns it as `keep_alive`.
- **A closed run carries its span and its saving.** The engine writes `ended`
  with `saved_usd`. That is the reread price of the run's context, set only
  when the next real request began while the cache was still warm.

## Rationale

- **A parked wait spends real money.** Hiding it hid a cost the user could not
  judge later (ADR 0529's reason for showing a status at all).
- **One fact, one phrasing.** The client formats and decides nothing, so the
  three surfaces cannot disagree.
- **The engine says how a run ended.** A client cannot tell a warm wake from a
  cold one, so the saving is the engine's to claim.

## Consequences

- Old events migrate to the new shape instead of keeping serde aliases. A
  `not_kept` with reason `turned_off` becomes `turned_off`. A clear after a run
  that pinged becomes that run's `ended`, with no saving, since it was never
  measured.
- A status of `codex`, `off` or `nothing_pending` is never recorded. The
  cache-warmth route reports it.
- A stopped, turned-off or never-pinged status is cleared when its wait ends. A
  run that pinged ends.
- The pill's explainer no longer names the beat route (copy or prefill).
- A run that pinged ends whenever the engine stops keeping its wait warm: the
  mode turns off, or a `not_kept` is recorded. Only `stopped` and `turned_off`
  carry the run on. When the same wait warms again, the client reopens the
  ended run's card rather than opening a second one.
- The tick's close waits while the thread runs and its wake's capture has not
  landed. So the end carries the wake's start and saving, never a guess.
- A migrated turn-off followed by more `not_kept` rows was wrongly ended. A
  follow-up migration turns it back into a clear, since the one-grammar
  migration may already be applied and is never edited.

## Alternatives considered

- **Serde aliases for the old shapes.** Rejected: every reader would carry two
  shapes for ever. A migration rewrites the rows once.
- **A client-side "Ended" inference, kept.** Rejected: it cannot carry the span
  or the saving.
- **A second status type for the pill.** Rejected: it is the drift this
  decision removes.
