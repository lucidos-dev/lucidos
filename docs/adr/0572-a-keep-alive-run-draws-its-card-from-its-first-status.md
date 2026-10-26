# 0572: A keep-alive run draws its card from its first warming status, before any ping, and a run that ends before its first ping closes as ended

- **Status**: Accepted
- **Date**: 2026-10-25
- **Amends**: [0540](0540-a-keep-alive-run-leaves-one-card.md). Its "at its
  first ping" and "a wait that ended before its first ping leaves no card" no
  longer hold.

## Context

ADR 0540 drew a run's card at its first ping, since a wait that ended earlier
spent nothing. A thread parked on a question waits most of a ping window before
the first ping. One user answered 70 seconds into a 4.5-minute window. They saw
no keep-alive card and no explainer. They could not turn the keep-alive off
until a ping had gone out.

## Decision

A run draws its card from its first warming status, under a parked question or
permission card and as a turnless card alike. The card, its explainer, its math
and its switch all work at zero pings. A run that ends before its first ping
closes as `ended` with zero pings and no `saved_usd`.

## Rationale

- **The explainer is most useful before the first ping.** That is when the user
  wonders what the engine is about to spend.
- **A card with a final state beats a vanished one.** The ended card says the
  run cost nothing, instead of leaving the user to guess.
- **The reply read a warm cache with or without a keep-alive.** A zero-ping run
  saved nothing, so the engine sets no `saved_usd` and the score nets zero.
- **One card per run still holds.** The fold keys on the run's first event id
  and its `since`, as before.

## Consequences

- A wait the keep-alive never pinged now leaves a card where it left none.
- The engine closes a zero-ping run as `ended` at once, with no wake to wait
  for, instead of clearing its status.
- The switch works at zero pings. A run turned off then keeps its card, which
  can be turned back on.

## Alternatives considered

- **Draw the card at the first ping and add a separate pre-ping surface.**
  Rejected: two surfaces for one run, and the explainer would sit on a card
  that is not there yet.
- **Keep clearing a zero-ping run.** Rejected: the card would then freeze on
  "warming" in the transcript, since nothing closes it.
