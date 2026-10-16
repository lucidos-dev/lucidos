# 0432: Home is one place with two faces; board widgets take one of three sizes

- **Status**: Accepted
- **Date**: 2026-10-11

Amends [ADR 0407](0407-inline-widget-cards-and-an-opt-in-shelf.md) (Home's
pins), [ADR 0411](0411-every-workspace-has-home.md) (where a cold start lands)
and [ADR 0419](0419-widget-windows-stay-open.md) (the tap in Home).

## Context

A tester asked for widgets that are always visible, with live updates, sizes
and reordering, and for a widget API that plugins can build on. A widget
pinned to Home was one long press away (ADR 0419), but never in view.

Three gaps shaped the answer. The content pane is never empty, so "Home when
nothing is open" has no slot to fill. No widget has a size: a frame is as tall
as the content it reports. And nothing in the app reorders by drag.

Plan: `docs/plans/2026-10-11-home-widget-surface.md`.

## Decision

**Home is one place with two faces: the *Home board* and the Home chat.** The
board shows the widgets pinned to Home in a grid. The Home chat is the Home
thread, unchanged. A phone shows the two faces in one pane, the chat sliding
up over the board. A desktop shows the chat in the Conversation and the board
in the Canvas, where it stays while other threads open.

**A board widget takes one of three *widget sizes*:** `small` (1 × 1 cells),
`medium` (2 × 1) and `large` (2 × 2). A widget may declare the sizes it
supports in its app manifest, and the first is its default. Amended: none
declared first meant `["medium"]`, so no tile could be resized and the
S · M · L pill never showed. It now means all three, starting medium, as
Apple's WidgetKit does. A declared list narrows the choice.

**The arrangement is a snapshot per *layout class*.** `HomeBoardArranged {
layout, tiles }` on the Home thread records the order, sizes and hidden tiles
of one layout class, `phone` or `desktop`. The newest one per layout wins. The
tiles themselves are Home's pins, with no second store.

## Rationale

- **One meaning of Home.** A separate board view would give two things called
  Home. Making the Home thread Home's chat renames nothing and changes nothing
  about its memory, voice or reach.
- **Always visible needs a place that outlives the open thread.** On desktop
  the Canvas holds the board while the Conversation changes. Its size comes
  from the divider, never from what the Conversation shows.
- **Three named sizes keep the widget author's job small.** Free sizes or
  arbitrary spans mean every widget must handle every shape. The three match
  the iOS families, so an OS home-screen ramp could map them one to one.
- **Square cells in rem.** The board computes its columns from its own width,
  never fewer than two. So one declaration works on every screen and at every
  UI scale.
- **Fixed cells, amended.** Cells first stretched to share the width, so
  tiles jumped by up to half again as the rail was dragged. Now a cell keeps
  its edge at every width, as on iOS and Android home screens. A narrower
  board drops a column, and the spare width splits evenly on both sides.
  Only a board too narrow for two cells, as on a phone, draws them smaller.
  Plan: `docs/plans/2026-10-14-home-board-fixed-cells-all-sizes.md`.
- **A snapshot, not per-move events.** Two devices arranging at once resolve
  cleanly: the last one wins, whole. Interleaved moves would give a layout
  nobody made.
- **Per layout class, synced.** A phone wants other sizes than a desktop, and
  a layout kept in one browser would be out of the agent's reach.

## Consequences

- **ADR 0407:** a pin in Home now also puts a tile on the board. Pin to Home
  on another thread's own widget makes it reusable first, in the same action,
  so deleting that thread never silently empties a tile.
- **ADR 0411:** a device with no stored focus opens Home on its board face. A
  device with no saved Canvas panel opens the Home board there. A saved panel
  is kept. The Home thread stays durable and never ephemeral.
- **ADR 0419:** a tap on the Home icon while in Home reveals the board, on a
  phone by sliding the chat down. A hold still lists Home's widgets, and the
  menu gains Open Home board as its first row. This also replaces ADR 0362's
  amendment that a tap in Home lists the widgets.
- **Reordering never moves an iframe in the DOM.** Tiles keep their DOM order
  and take their board position from CSS `order`.
- **A tile's size comes from its class, never its content.** The frame fills
  the tile, and content past it is clipped under a fade. Amended: the tile
  drew an Open control over the fade until its menu's Open in Canvas made it
  redundant.
- **Arranging is prompt-first.** The `widgets` tool, the CLI and HTTP all
  write the same event. The agent arranges Home only when the user asks in
  that turn, and never chooses a size for a new pin: a pin takes no size.
- **Frames relay only the event types they listen to.** A board of live tiles
  would otherwise multiply every streaming delta. A `*` listener keeps the old
  behaviour.
- **No tile cap yet.** Only tiles on screen mount. A cap per layout class
  comes only if measured memory shows one is needed.

## Alternatives considered

- **The content pane when no thread is open.** It needs an empty slot that
  does not exist, and an ephemeral Home chat that breaks ADR 0411.
- **A widget area inside the Home thread.** Visible only while Home is open,
  so not always visible, and a dashboard inside a transcript.
- **A separate board view with a name of its own.** Two meanings of Home, or
  a board unrelated to Home. The maintainer found that confusing.
- **Free pixel sizes, or grid spans of any width and height.** Too many shapes
  for an author to design and test.
- **A fourth size.** Not asked for. Three hold until someone needs more.
- **Cells that stretch to fill the width**, the first design. Every tile
  changes size whenever the board's width crosses a column boundary.
- **A layout per device in `localStorage`.** A new browser starts empty, and
  the agent cannot arrange it.
- **One layout per workspace.** A phone and a desktop want different sizes.
- **A static snapshot per tile, rendered server side.** It needs a renderer in
  the shell, which ADR 0402 rejected, and it goes stale between pushes.
- **A greeting or digest on the empty Home chat.** Deferred: the chat stays as
  it is for now.
