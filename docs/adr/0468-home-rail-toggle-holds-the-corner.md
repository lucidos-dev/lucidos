# 0468: The Home rail's toggle holds the window corner in both states, and the board's head drops its title

- **Status**: Accepted
- **Date**: 2026-10-13
- **Amended by**: [ADR 0561](0561-dashboard-head-titled-on-desktop.md), decision 2

Amends [ADR 0467](0467-home-rail-toggle-sits-beside-the-bell.md), decision 1.

## Context

With the rail shown, the toggle slid left to lead the rail's head, before the
"Home board" title. In use, the maintainer asked for the toggle to stay at the
far right when the rail is open too, and for a centred title. Rendered, a
centred title ran into Add and Edit on a narrow rail. The maintainer then asked
whether the head needs a title at all.

## Decision

1. **The toggle holds the window corner in both rail states.** Shown, the
   rail's head keeps room for it at its trailing end. Hidden, the Canvas row
   does, as before. It never moves.
2. **The rail's head has no title.** Add and Edit sit at its trailing end,
   before the toggle. The rail keeps its accessible name, "Home board". The
   phone's Home pane keeps its title, since nothing else there names the board.

## Rationale

- **A control that never moves is the one the hand finds.** The maintainer
  picked it from rendered options, over keeping it beside the bell.
- **The title said what the rail already shows.** Without it, nothing competes
  for room on a narrow rail, so no centring or clamp is needed.

## Consequences

- The toggle no longer slides, so its slot transitions only its width, for the
  shrink when it is not offered.
- The board's one head now draws its title by layout, on the phone only.

## Alternatives considered

- **Centre the title, with Add and Edit before the toggle.** Rejected: the
  title moved off centre and shortened on a narrow rail.
- **Centre the title between Add on the left and Edit on the right.** Better
  balanced, but the title still shortened at the floor width.
- **No title, Add on the left.** Rejected by the maintainer from rendered
  options, in favour of both buttons together at the trailing end.
