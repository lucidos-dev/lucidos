# 0632: An inline widget is a bare widget box centred in its turn, and its size and actions live in the widget menu, amending point 2 of ADR 0621

- **Status**: Accepted
- **Date**: 2026-11-09

Amends point 2 of [ADR 0621](0621-widgets-one-box-no-pin.md) (the thread card
is a bar over a widget box). The rest of ADR 0621 stands.

## Context

ADR 0621 made the thread card a tile: the same widget box a dashboard tile
draws. It kept a bar over the box, holding the icon, the name, the S · M · L
pill, "+ Dashboard" and the ⋯ menu.

In use, the card read as a framed panel, not as a tile. It sat at the turn's
left edge, close to the text around it. The bar drew a second surface over the
widget, which usually opens with its own name. The user asked for the card to
look as close to a dashboard tile as possible: centred, with room above and
below, and its controls behind a right-click or a long press.

Two things already made that possible. "Add to dashboard" is a menu item
(`DashboardItem`). Frames forward an unclaimed right-click to the host
([ADR 0528](0528-widget-frames-forward-right-click.md)).

Plan: `docs/plans/2026-11-09-frameless-centred-inline-widgets.md`.

## Decision

An inline widget is its bare widget box, drawn as a dashboard tile is: a tile's
border and radius, no shadow, no bar. It centres in its turn, with
`--space-xl` above and below. A phone box wider than the turn still centres.

The widget menu carries what the bar did. Its first row is Size, holding the
shared `WidgetSizePill`, above a divider. A right-click opens it at the
pointer, and a long press opens it against the card. The card is a focus
stop, so the menu key or Shift+F10 opens it from the keyboard, Mac included,
as on an app shortcut tile. The items are the same on
every device. The SDK's hold-to-menu, which ran only on a dashboard tile, runs
on an inline card too.

## Rationale

- **One look for one thing.** The card and the tile are the same box from the
  same component. Drawing them alike says so, and what the thread shows is what
  the dashboard shows.
- **The widget is the content.** A bar named the widget a second time, above a
  widget that names itself. Without it the box is the answer.
- **One menu, every device.** The bar's controls were a second way to reach
  menu actions, and only on a wide card. A small card had already folded most
  of them into the ⋯.
- **A pick stays open.** Picking a size keeps the menu open on the new size, so
  the user can compare sizes without reopening it.

## Consequences

- No control is visible on the card. The menu is reached by right-click, long
  press, or the frame's forwarded context-menu key.
- The card no longer shows the widget's name or icon. Its name is its
  accessible label. Widget guidance (`building-an-app.md`) no longer leans on
  the bar for the name: the reply around the card says what it is.
- `AddToDashboardButton`, the `.widget-bar` styles and `useObjectMenu`'s
  `anchor` option go.

## Alternatives considered

- **Keep a bar, but hide it until hover.** Rejected: a phone has no hover, so
  the card would differ by device, and a hover-revealed bar still jumps the
  layout or covers the widget.
- **A floating ⋯ in the box's corner.** Rejected: it sits over the widget's own
  content and its tap targets, and a dashboard tile draws none.
- **Size as three menu items, not a pill.** Rejected: the pill is the control
  an edited dashboard tile already uses, so one control picks a size wherever
  a size is picked.
