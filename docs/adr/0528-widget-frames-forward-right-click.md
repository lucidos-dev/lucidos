# 0528: Widget frames forward an unclaimed right-click, and a tile hold opens the tile menu

- **Status**: Accepted
- **Date**: 2026-10-19

## Context

ADR 0285 made an object's right-click open its own ⋯ menu. A widget is such an
object: an inline widget, the open shelf widget and a dashboard tile each carry
a ⋯ menu. But most of a widget is its frame. ADR 0285 left app iframes to own
their own right-click, since a `contextmenu` event does not cross a document
boundary. So a right-click on the bar opened the menu, and a right-click a few
pixels lower showed WebKit's page menu, with Reload in it.

The dashboard had its own meanings on top. A right-click or a finger held on a
tile entered edit mode. So the same gesture meant one thing on a tile and
another on every other widget.

## Decision

A widget frame the host marks `menu` forwards an unclaimed right-click to the
host, which opens the widget's own menu at the pointer. The frame decides
"unclaimed" with the host's own rule, shared from the SDK package. A finger held
on a dashboard tile opens the tile menu too, whose Edit dashboard enters edit
mode.

This amends ADR 0285's consequence that app iframes own their own right-click.
It still holds for a frame that is not a widget, and for a widget that claims
the event.

## Rationale

- **One rule on both sides of the frame edge.** The frame runs
  `shouldSuppressContextMenu`, the same function the host runs. Content inside
  a widget keeps the native menu: a link, an input, media, a selection, and the
  text of `pre`, `code` or `[data-native-context-menu]`. Alt+right-click keeps it
  everywhere. With one definition, the host and a frame cannot disagree.
- **A widget can still own its right-click.** The forwarder listens on `window`
  in the bubble phase, so a widget handler that calls `preventDefault` runs
  first and wins.
- **The host says which frames carry a menu.** A question option's widget and an
  app in the Canvas have no widget menu, so they are not marked and forward
  nothing.
- **Hold opens the menu, as on an iOS home screen.** There a held icon shows its
  menu, which ends in Edit Home Screen. Edit mode stays one tap away in the tile
  menu and in the dashboard header, and every widget now gives the gesture one
  meaning.

## Consequences

- A new widget surface passes `onMenu` to `WidgetFrame`, which marks the frame.
  The host translates the frame point to the viewport and clamps it.
- A forwarded request from a frame behind the top overlay is dropped, so at
  most one menu is open. A frame inside the top overlay, such as the open shelf
  widget, may still open its menu.
- No hold inside an inline or shelf frame: a finger there is usually scrolling
  or using the widget. Touch reaches the menu through the ⋯ or a hold on the bar.
- A widget that relied on the WebKit page menu inside its frame loses it, short
  of Alt+right-click.

## Alternatives considered

- **Leave frames alone.** Rejected: the right-click works on a sliver of bar
  and fails on the rest of the widget, which reads as broken.
- **An overlay over the frame that catches right-clicks.** Rejected: it would
  also catch every click, scroll and text selection the widget needs.
- **Keep the tile hold entering edit mode.** Rejected: it gives one gesture two
  meanings across widgets, and it skips the tile's own actions.
