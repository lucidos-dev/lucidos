# 0540: A cache keep-alive run leaves one turnless card in the transcript, not an end-of-feed divider

- **Status**: Accepted
- **Date**: 2026-10-20
- **Amends**: [0529](0529-keep-alive-shows-one-status.md). Its dashed divider
  and "The status is live only" no longer hold on the client. Each ping still
  leaves no trace, and the engine still emits one status per change.
- **Amended by**: [0563](0563-cache-keep-alive-speaks-one-grammar.md). A parked
  thread shows its run under the card it waits on, and an ended run reopens
  when the same wait warms again.
- **Amended by**: [0572](0572-a-keep-alive-run-draws-its-card-from-its-first-status.md).
  The card opens at the run's first warming status, before any ping.

## Context

ADR 0529 drew the *cache keep-alive status* as a divider at the end of the
feed, read from the thread meta. The divider hid the moment the thread ran,
and the engine clears the status once the wait ends. A keep-alive that cost
real money left nothing behind, unlike most events in the transcript.

## Decision

Each *keep-alive run* (one wait the engine kept warm) leaves one turnless
engine card in the transcript, at its first ping. Later statuses of the run
update that card in place. The card stays after the run ends.

## Rationale

- **A record that vanishes cannot be judged later.** The user sees what the
  engine spent, after the thread moved on.
- **One card per run, not per ping.** ADR 0529's reason for hiding each ping
  still holds, so the run folds into a single card.
- **No engine change.** The events already carry every field the card shows.
  The client folds them (`foldKeepAliveStatus`).
- **The card counts down only while its run is open.** It reads the thread's
  shown status and matches the run by `since`, so an old card never ticks.

## Consequences

- The end-of-feed divider goes. The waiting panel's "Prompt cache" section and
  the cache-warm line stay.
- A cleared status, or a ping count that fell back to a lower number, closes
  the run. A `not_kept` status leaves it open, since the engine counts pings
  from the wait's start and the same wait may warm again. A wait that ended before its first ping leaves no card.
- The card is a *turnless boundary*, like `ChildReportFiled`: it opens an
  exchange, holds no response and hands the running turn straight back.

## Alternatives considered

- **Keep the divider and stop hiding it mid-turn.** Rejected: the engine
  clears the status at the wait's end, so the record would still vanish.
- **A dashed line in the transcript instead of a card.** Rejected: the user
  chose a card with the engine glyph and label, like its sibling events.
- **One card per status event.** Rejected: about one per ping, which buries the
  conversation (ADR 0529).
