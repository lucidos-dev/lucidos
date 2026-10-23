# 0466: The Home board is a desktop rail; widget windows, the dock and the Home long press are gone; one inline widget

- **Status**: Accepted
- **Date**: 2026-10-13

Supersedes [ADR 0419](0419-widget-windows-stay-open.md). Amends
[ADR 0415](0415-widgets-take-params-and-embed-where-pictures-draw.md) (the card
and the embed), [ADR 0432](0432-home-board-two-faces-three-sizes.md) (where the
desktop board lives) and
[ADR 0455](0455-desktop-home-chat-and-board-get-a-button-each.md) (the board
button and the tap in Home).
Plan: `docs/plans/2026-10-13-home-board-rail-and-widget-surface-cut.md`.

## Context

The maintainer found the widget dock band "weird" and asked whether it belonged
above the header or beside everything. Rendered options showed the band under
the header took height from every pane, and left empty space beside a narrow
widget. A band above the header put the macOS traffic lights on a widget.

The question then widened. An audit found three desktop surfaces doing one job,
keeping Home's widgets in view: the Home board in the Canvas, floating widget
windows, and the dock band. One Home widget could be on screen four times at
once. The maintainer asked two things. Should the board itself be that
always-there surface? And are the widget card and the widget embed not the
same thing?

## Decision

1. **On desktop, the Home board is a rail at the window's right edge.** It runs
   the full window height, outside the header and the panes, which take the
   width left of it. Its divider resizes it, clamped at two board cells and at
   the pane floors. The board button beside the Canvas menu hides and shows it.
   Its width and shown state persist per device. The Canvas has no board view.
2. **The phone is unchanged.** The board is Home's face in its middle pane.
3. **Cut**: widget windows in all three modes, the widget dock and its toggle,
   the Home long-press widget list, and the hover peek. A tap on Home opens
   the Home chat. In Home, a desktop tap does nothing more: only the rail's
   toggle shows the rail. (Amended after the maintainer asked that the Home
   icon never open the board.)
4. **One inline widget.** The card and the embed merge into one, written as an
   embed. The `widgets show` action goes.
5. **"Add widget" sits in the thread's menu** and pins a reusable widget to that
   thread's shelf, through the board's picker.

## Rationale

- **A desktop has width to spare and little height.** A rail takes width and
  gives a tall widget its full height. A band takes height from every pane.
- **One place per job.** The board already had sizes, arrangement, sync across
  devices and the agent's `arrange`. Windows and the dock re-did its job with a
  per-device layout of their own, which no other device or agent could reach.
- **Outside the header, never over it.** The rail stops the header where it
  begins, so every header row keeps centring on its own pane. The right edge
  holds no OS chrome, unlike the top-left corner.
- **Inside `.app-shell`.** An open overlay inerts the rail with the rest of the
  shell, so it needs no exception to the outside-tap rule. ADR 0419's
  exception goes with the windows.
- **Same thing, one name.** A card and an embed both draw a live widget in a
  turn. Two existed only because a question option cannot hold a tool call.

## Consequences

- `--z-widget-window`, the dock's height term in `--app-header-bottom`, the
  SDK's `data-widget-mode`, its width report, its drag pass-up and the `window`
  and `embed` places are gone. A widget sees `home`, `inline`, `shelf` or
  `option`; a reply's embed draws as `inline`.
- A per-device `localStorage` key holds the rail's width and another its shown
  state. The old window keys stay in browsers, unread.
- With the rail shown, the header and panes are narrower by its width. Showing
  it re-runs the split's floor migration, as Home's arrival does. On a narrow
  window the pane floors stop fitting sooner, which `clampSplitRatio`'s
  empty-range branch already handles.
- Nothing writes `WidgetShown` now, so the workspace prompt footprint counts a
  reusable widget as used when a reply embeds it or a pin names it.
- The agent's `home-board` navigation target shows the rail on desktop, and
  lands on Home's board face on a phone.

## Alternatives considered

- **Keep the band, let the user pick its position.** Rendered at top, bottom,
  left and right. Lost: the maintainer saw it repeated the board, and a top band
  puts the traffic lights on a widget.
- **Keep floating windows beside the rail.** Offered as a fork. Lost: the rail
  is always there and hideable, so a window re-does its job.
- **Merge card and embed in a separate change.** Offered. The maintainer chose
  one plan, split at implementation if it grows too large.
