# 0396: The drawer grouping moves from a title band to one header button that shows where a tap goes

- **Status**: Accepted, amended by [0401](0401-drawer-ongoing-tile-selector.md) (Ongoing draws all four tiles, empty ones too)
- **Date**: 2026-10-09
- **Supersedes**: the title band of ADR 0393

## Context

ADR 0393 picked the drawer grouping with a band of two words, Folders and
Ongoing, under the threads header. The band cost a whole row above the list.
The user asked for icons instead, with Filter beside them. The desktop drawer
header had no room for two more icons: at the drawer floor they cut the title
down to an ellipsis.

Ongoing also drew every group, empty ones dimmed with a 0. With most groups
empty, the list read as four headers and a blank pane.

## Decision

One header button swaps the grouping. It shows the grouping a tap goes to: a
folder while Ongoing is on screen, an inbox while Folders is. The band row is
gone. Filter leads the header row and shows under Folders only. The title names
the grouping and hides when it cannot fit whole. Ongoing draws only the groups
that hold a thread.

## Rationale

- **One slot, not two.** Two icons that detoggle each other are a two-state
  control spread over two boxes. One box does the same job, and the header
  keeps two controls at each end.
- **Destination, not state.** A single button that swaps between two states
  shows the action, like play and pause. The title already says where you are,
  so an icon that repeats it says nothing. It also puts the attention badge on
  the inbox under Folders: "this many need you over there".
- **Whole or hidden.** An ellipsized "Threa…" reads as a bug. A missing title
  reads as a narrow pane, and the button glyphs still say where you are.
- **Empty groups hidden.** A header with nothing under it is a dead control.

## Consequences

- The drawer floor (`computeMinDrawerWidth`) pays two controls at each end and
  no title room. That is 320px at 100%, up from 312.
- On the packaged macOS build the traffic-lights reserve leaves no title room
  at the floor, so the title hides there until the drawer is widened.
- The drawer holds only its tree, so its tab cycle has one stop.

## Alternatives considered

- **Two icons in the header.** No room at the floor, as above.
- **Icons in the band row.** It keeps the row the user wanted back.
- **The icon shows the current grouping.** It repeats the title, and leaves the
  badge on the folder, away from the threads it counts.
- **Ellipsize the title.** Rejected by the user in favour of hiding it.
- **Keep empty groups with a 0** (ADR 0393). Its worry was header order
  shifting as threads come and go. The order stays fixed; only presence
  changes. A 0 row answers a question the user did not ask, at the cost of a
  screenful of dead headers.
