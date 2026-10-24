# 0467: The Home rail's toggle sits right of the bell and slides with the rail; separator and head match the panes

- **Status**: Accepted
- **Date**: 2026-10-13
- **Amended by**: [ADR 0468](0468-home-rail-toggle-holds-the-corner.md),
  decision 1. The toggle now holds the window corner in both states.
  [ADR 0484](0484-home-rail-toggle-always-offered.md), decision 2. The toggle
  is now offered whenever Home exists, an empty board included.

Amends [ADR 0466](0466-home-board-is-a-desktop-rail.md) (the board button that
hides and shows the rail).
Plan: `docs/plans/2026-10-13-home-rail-corner-toggle.md`.

## Context

The maintainer asked for a Home board toggle like the thread drawer's, "placed
to right of notifications bell if collapsed (and has content)". The rail's
separator was a 1px line, and its head drew a hairline that no other header
draws.

## Decision

1. **One toggle, right of the bell in both rail states.** Shown, it leads the
   rail's head, before the title. Hidden, it rests in the window's corner, in
   the Canvas row, which keeps room for it. It slides between the two on the
   rail's curve. The glyph is the sidebar glyph, mirrored.
2. **Offered while the rail shows, or while it is hidden and the board has a
   tile.** Otherwise, and when the Canvas collapses with the rail hidden, it
   shrinks away in place.
3. **The board button beside the Canvas hamburger goes.**
4. **The separator is a pane divider band, and the head has no hairline.** The
   head spans the whole bar, the macOS title-bar band included, so its controls
   centre on the header's line.

## Rationale

- **The toggle stays next to the bell.** The maintainer picked it from rendered
  options. The eye finds it at the same neighbour in both states.
- **A hidden, empty rail has nothing to show.** The toggle would only open an
  empty board.
- **One control, never two.** A second copy crossfading in the Canvas row is the
  ghost `frontend.md` forbids. A single element moving by `right` is the shape
  that rule allows.

## Consequences

- The Canvas title's reserve grows by one box and a gap while the toggle is
  docked, and the header's action collapse charges that padding.
- `--header-padding-x` lives on `:root`, since the toggle sits outside the header.
- Showing the rail with the Canvas collapsed needs the Canvas back first, as it
  did when the button lived in the Canvas row.

## Alternatives considered

- **A fixed corner, mirroring the drawer toggle (ADR 0276).** Rejected by the
  maintainer from rendered options. Shown, it sat past Edit, away from the bell.
- **Keep the board button beside the hamburger.** Rejected: the request was a
  toggle at the rail's own edge.
