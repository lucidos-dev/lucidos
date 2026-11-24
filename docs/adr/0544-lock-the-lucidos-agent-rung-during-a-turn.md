# 0544: The Lucidos Agent saves rung picks for the next turn and keeps the running rung visible

- **Status**: Accepted
- **Date**: 2026-10-21
- **Amended**: 2026-11-15

## Context

A Lucidos Agent thread's *rung control* took a pick while a turn ran. It then
named the picked pair, while the turn ran on to its end on the old one.

The pick itself lived in the tab's memory until the next send, which carried it
as an override. So a reload or another device dropped it. A turn the engine
started itself, for a child's report or a wake, ran the thread's recorded pair
while the pill named the pending pick.

ADR 0542 locks a coding-agent thread's rung while a turn runs, and records a
pick between turns on the engine. The user asked for the Lucidos Agent to match
it in full.

## Decision

A Lucidos Agent rung pick is saved immediately through
`POST /api/v1/chat/settings`, including while a turn runs. It applies to the
next turn. The running turn keeps its model, effort and provider.

This amends the original refusal policy: users should be able to pick during
work without remembering to return when it finishes. ADR 0542's coding-agent
policy stays unchanged.

- **Picks are durable.** `ChatSettingsChanged` records the resolved pair. The
  latest accepted pick replaces earlier picks and never changes account defaults.
- **Applied settings are distinct.** At admission, the engine records a
  `ChatTurnSettingsResolved` with `request_event_id` naming that turn's anchor.
  `ChatSettingsChanged` remains exclusive to user picks, preserving subscriptions.
  This also covers continuations whose starter carries no model.
- **The gate orders starts and picks.** A turn holds its per-thread start gate
  through resolution and the applied stamp. A pick waits for this short section,
  then records next-turn settings. It never waits for the full running turn.
- **Queued turns release the gate while waiting.** After admission they reacquire
  it and resolve again. The latest pick therefore reaches the queued turn.
  Explicit send overrides retain their precedence.
- **Injection keeps the live pair.** An injected `MessageReceived` names the
  running pair and carries the live request anchor. Settings queries and paged
  pair snapshots exclude these messages, so they cannot erase a pending pick.
- **Explicit overrides are never silently dropped.** An HTTP send that asks
  for a different live pair is refused before recording. If a turn starts after
  that check, process admission queues the message with its override intact.
- **The pill stays truthful.** Both desktop and compact faces retain the
  running rung and add `*` while a pick is pending. The popover explains it
  with “Running” and “Next turn” pairs. When the turn ends, the saved pair
  becomes the face. A later pick replaces the pending explanation.
- **Reload and other devices agree.** Events and paged pair stamps carry both
  the applied pair and the pending pick. The browser holds no authoritative
  pending selection. Follow-up sends do not turn implicit settings into overrides.

## Rationale

The next turn must read the pick wherever that turn starts: a send, queued
follow-up, continuation, child report or wake. Browser-only state cannot do that.

Saving a pick does not make it the running model. Distinguishing applied
settings from picks preserves honest attribution while allowing useful mid-turn
interaction. An asterisk keeps both collapsed faces compact; the popover gives
the full explanation.

The short start gate provides a definite ordering without blocking picks behind
queued messages. Re-resolving after admission prevents a queued turn from
retaining the settings it read before waiting.

## Consequences

- Each admitted Lucidos Agent turn adds a `ChatTurnSettingsResolved` event. Existing
  histories fall back to their non-injected starter stamps.
- Pair snapshots retain the applied pair as well as the newest selection.
- The shared rung component supports a pending marker. Coding-agent picks
  retain their existing lock behavior.
- Compose drafts keep their per-draft overrides (ADR 0437), bound at first send.
- Coverage includes engine concurrency and override regressions, event replay,
  paged snapshots, and desktop and phone browser layouts.

## Alternatives considered

- **Refuse mid-turn picks.** This was the original policy. It lost the user's
  intent and required another visit after the turn ended.
- **Show the pending model as running.** It misattributes the current answer.
- **Keep pending settings in the browser.** Reload, other devices and engine
  re-entries would lose them.
- **Write an account preference.** A per-thread pick must not change other threads.
