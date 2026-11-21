# 0633: XL widget size: a fourth size, opt-in, 4 by 2 cells, scrolling sideways where the host is narrower

- **Status**: Accepted. Amends the size table of [ADR 0432](0432-home-board-two-faces-three-sizes.md) and the one-box contract of [ADR 0621](0621-widgets-one-box-no-pin.md)
- **Date**: 2026-11-09

## Context

A widget could take `small`, `medium` or `large`. Large is two cells square, so
a widget that wants a wide, short or data-rich face had no size to ask for. The
user asked for a fourth size while reviewing inline widget cards in a thread.

A cell is 10rem with a 0.75rem gap. A dashboard has at least two columns and a
phone has exactly two. A thread card takes the cell its pane's width gives, and
the turn column stops at 50rem.

Plan: `docs/plans/2026-11-09-xl-widget-size.md`.

## Decision

**`xl` is a fourth *widget size*: four cells wide and two high, 42.75 by
20.75rem at full cell size.** It is opt-in. A widget that declares no sizes
still takes small, medium and large, and a declared `xl` validates.

**Where an XL box does not fit, the host clamps the tile or card and scrolls the
box sideways.** The widget keeps its true XL width and always sees `xl`. A
dashboard tile spans at most the grid's steps, and a card stops at its turn's
width. The widget never scrolls itself: the host's viewport onto the box does.
The host tells a clamped box's frame that it pans. The SDK's pane swipe then
leaves a sideways drag to the browser's native pan, which reaches the scroller.

## Rationale

- **One box everywhere.** The size table (`WIDGET_SIZE_CELLS`) stays the one
  source for a card and a tile, so an XL card and an XL tile are one box.
- **Opt-in protects existing widgets.** An XL box is wider than any turn on a
  phone. A widget written for three sizes would stretch across it.
- **Scrolling keeps the contract honest.** A widget that reported `large` in a
  narrow pane would hide the size the user picked. The scroller moves the
  problem to the host, which knows its own width.

## Consequences

- Four size lists change together: the Rust enums, the wire type, the SDK
  constants and the capability schemas. A pin test compares the capability
  list to `WidgetSize::ALL`.
- A narrow host shows part of an XL box until the user scrolls it. That is the
  price of keeping the size the user chose.
- The built-in video player declares `medium`, `large` and `xl`.

## Alternatives considered

- **Make XL a fixed 3 by 2.** It fits a three-column dashboard but not a
  phone, so it would need the same clamp and gives less room.
- **Fall back to `large` where it does not fit.** The user rejected it: the
  card would show a size nobody picked.
- **Offer XL to every widget.** It would stretch widgets written for three
  sizes, so it is opt-in.
- **Gate the size pill on available width.** A pane can be resized, so the pill
  would change under the user. Scrolling covers every width.
