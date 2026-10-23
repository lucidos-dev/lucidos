# 0455: Desktop: the Home icon opens the Home chat, the Home board has its own button

- **Status**: Accepted
- **Date**: 2026-10-12
- **Amended by**: [ADR 0466](0466-home-board-is-a-desktop-rail.md). The
  desktop board is a rail with its own toggle, so the board button beside the
  Canvas menu is gone. A tap on the Home icon opens only the chat, in Home too.

Amends [ADR 0432](0432-home-board-two-faces-three-sizes.md) (the tap on the
Home icon). Plan: `docs/plans/2026-10-12-desktop-home-chat-and-board-buttons.md`.

## Context

ADR 0432 made Home one place with two faces. On desktop, one tap on the Home
icon opened the Home chat in the Conversation and the Home board in the Canvas.
The maintainer found that weird: one button changed two panes. The board's own
entries also drew the house, so the two faces read as one thing.

## Decision

**On desktop, each face of Home gets its own button.** The house in the
Conversation header opens the Home chat and leaves the Canvas alone. A
tile-grid button beside the Canvas hamburger opens the Home board. Every board
entry draws the tile grid: that button, the menu drawer row, the history menu
and the widgets menu's Open Home board row.

**New thread leads the Conversation header, beside Home.** It no longer folds
into ⋯ with the trailing actions.

**In Home, a desktop tap on the Home icon lists Home's widgets.** There is
nothing left to open there, and a dead tap reads as broken. This restores what
a tap in Home did before ADR 0432 gave it the board.

**The phone was unchanged.** Home stayed one pane with two faces, and the Home
icon landed on the board face. ADR 0538 later made Home a plain chat there.

## Rationale

- **A button acts where it sits.** Each pane's header now opens what that pane
  shows, so a tap never changes a pane the pointer is not over.
- **One icon per concept.** The house means the Home chat and the tile grid
  means the board, so a glance tells them apart.
- **The phone already works.** It has one pane, and the Home icon flips
  between its faces. A second button there would add nothing.
- **Navigation and create lead the row.** Home and New thread are where you go
  and what you start, so they sit at the leading edge beside the drawer toggle.
  Tools such as Search stay at the trailing edge.

## Consequences

- ADR 0432's "one meaning of Home" stands: the board is still Home's board,
  and nothing is renamed.
- A device with no saved Canvas panel still opens the board there. A cold
  start still opens Home on its board face.
- The Conversation row's reserve and the pane's floor count New thread's box,
  so the workspace name beside the mark hides sooner on a narrow pane. The
  Conversation pane's floor grows by a box and a gap at each end. On a 1280px
  screen the three desktop floors stop fitting from 112.5% ui-scale, one step
  sooner than with Home alone (`store/paneMinimums.ts`).
- The long press and hover peek on Home still list its widgets.

## Alternatives considered

- **Keep the one tap opening both.** Rejected: it is the behaviour the
  maintainer reported as weird.
- **Put the board button beside Home in the Conversation header.** Rejected:
  it would sit over the chat but change the Canvas. It also cost the row a box,
  for no gain over the Canvas header.
- **Keep New thread in the trailing cluster.** Rejected by the maintainer: Home
  and New thread belong together, at the leading edge.
