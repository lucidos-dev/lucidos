# 0484: The Home rail's toggle is offered whenever Home exists, an empty board included

- **Status**: Accepted
- **Date**: 2026-10-15

Amends [ADR 0467](0467-home-rail-toggle-sits-beside-the-bell.md), decision 2.

## Context

ADR 0467 offered the toggle while the rail shows, or while it is hidden and the
board has a tile. A user who hid the rail on an empty board lost the toggle, and
with it the board's own Add. The only ways back were to pin a widget from
another thread, or to ask Lucidos to show the board. The maintainer asked for
the toggle to stay visible always.

## Decision

The toggle is offered whenever Home exists, whatever the board holds. It still
shrinks away while it is docked with the Canvas collapsed, as before.

## Rationale

- **An empty board is not nothing to show.** Its empty state carries the Add
  button, so opening it is how a user pins a first widget.
- **A control that vanishes with no trace reads as a missing feature.** The user
  found no way back and reported the toggle as broken.

## Consequences

- The Canvas row keeps room for the toggle whenever Home exists and the rail is
  hidden.
- `data-home-rail-toggle` now marks only that Home exists, so the slot's
  "not offered" shrink rule is gone.

## Alternatives considered

- **Keep the 0467 rule, and point at another route back.** Rejected: a route
  through chat or another thread's menu is undiscoverable from the empty rail.
