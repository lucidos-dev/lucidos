# 0561: The desktop dashboard rail's head shows the Dashboard title again, as on the phone

- **Status**: Accepted
- **Date**: 2026-10-23

Amends [ADR 0468](0468-home-rail-toggle-holds-the-corner.md), decision 2.

## Context

ADR 0468 dropped the title from the desktop rail's head, because a centred
title ran into Add and Edit on a narrow rail. The phone kept its title, drawn
leading, with Add and Edit at the trailing end. In use, the maintainer asked
for the same head on desktop.

## Decision

The dashboard's head draws the "Dashboard" title on both layouts. It leads the
row, and Add, Edit and the rail toggle keep the trailing end.

## Rationale

- **One dashboard, one head.** The two layouts now differ only in where the
  dashboard sits, not in how its head reads.
- **A leading title never needed centring.** The rejected options in ADR 0468
  were all centred. A leading title shortens with an ellipsis instead, so the
  buttons never move.

## Consequences

- At the rail's floor width the title shortens to an ellipsis first.
- `Dashboard.tsx` no longer branches on layout to draw the title.

## Alternatives considered

- **Keep the rail untitled (ADR 0468).** Rejected by the maintainer in use, who
  asked for the phone's head on desktop too.
- **Hide the title below some rail width.** Not needed: the ellipsis already
  yields its room to the buttons.
