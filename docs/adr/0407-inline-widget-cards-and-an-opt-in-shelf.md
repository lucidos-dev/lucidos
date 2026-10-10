# 0407: Inline widget cards fit their content and never scroll; the shelf holds only pinned widgets

- **Status**: Accepted
- **Date**: 2026-10-09

## Context

ADR 0405 took the widget frame out of the transcript, because an inline frame
that scrolled inside the transcript's scroller trapped a swipe. Every widget
then lived on the shelf, and the turn kept a one-line row.

In use, that was the wrong split. A widget is usually a one-off answer, and a
thread can make many of them. They belong in the conversation, next to the text
they answer. Few are worth a chip in the title row. OpenAI's Intelligent UI,
launched the same week, puts its widgets inline for the same reason.

The swipe trap came from the inner scroller, not from being inline. OpenAI's
Apps SDK rule says an inline card auto-fits its content and never scrolls
inside, with an expand control for anything bigger.

Plan: `docs/plans/2026-10-09-inline-widget-cards-motion-and-live-data.md`.

## Decision

A widget shows inline at its turn as a card: its bar with the widget under it.
The frame is as tall as the content the widget reports, so it never scrolls.
Past about one phone screen, the card clips with a fade and an Expand button
that opens the widget full size in Canvas.

The shelf is opt-in. `WidgetShown` adds no chip. A chip appears when the user
or the agent pins the widget (`WidgetPinned`), and goes when it is unpinned
(`WidgetUnpinned`). The old `WidgetRestored` and `WidgetHidden` read as those.

This amends ADR 0405's placement. ADR 0402's storage, events, reach and
lifecycle stand.

Home's pinned widgets also open from outside Home: a long press on a Home entry
lists them, and a pick floats the widget in a window
([ADR 0419](0419-widget-windows-stay-open.md)). The shelf's chips and drop are
unchanged.

## Rationale

- **No inner scroller, so no swipe trap.** A frame as tall as its content has
  nothing to scroll, and the card's clip is the host's, not the frame's.
- **The answer sits with the question.** A one-off widget reads in place, like
  the text around it.
- **The title row stays clean.** A thread of ten one-offs keeps an empty shelf;
  the picker the user returns to gets a chip.
- **Expand works for any widget.** Canvas opens a widget by id, pinned or not.

## Consequences

- Inline frames mount only while on screen (ADR 0227), as before ADR 0405.
  Superseded by ADR 0422: a frame now stays loaded a while off screen.
- Widgets shown before this change leave the shelf until pinned. Their cards
  stay in the transcript.
- The `widgets` tool and CLI rename `hide` and `restore` to `unpin` and `pin`.
- The agent's guidance asks for widgets that fit one screen, animate what a tap
  changes, and read live data through the SDK.

## Alternatives considered

- **Keep ADR 0405's shelf-only placement.** Lost on where a one-off answer
  belongs, and on a title row crowded by chips nobody returns to.
- **Inline cards with an inner scroll past a cap.** The ADR 0402 shape. Lost on
  the swipe trap ADR 0405 recorded.
- **Every widget on the shelf as well as inline.** Lost on the crowded title
  row; pinning keeps the chips that earn their place.
- **Keep the event names `WidgetRestored` and `WidgetHidden`.** "Restored" is
  false for a widget that was never on the shelf. Lost on names that describe
  what is true now.
