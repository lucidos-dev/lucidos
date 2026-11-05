# 0621: A widget draws in one widget box wherever it has a size; the thread card is a tile, pin to thread is gone, and Add to dashboard replaces it

- **Status**: Accepted
- **Date**: 2026-11-04

Supersedes [ADR 0407](0407-inline-widget-cards-and-an-opt-in-shelf.md) (inline
cards that fit their content, and the opt-in shelf), and with it what remained
of [ADR 0405](0405-widget-opens-from-the-shelf.md).
Plan: `docs/plans/2026-11-03-widgets-one-render-no-pin.md`.

Amends:

- [ADR 0402](0402-widget-is-an-app-kind.md): the shelf chip.
- [ADR 0414](0414-app-icons-and-plugin-media.md): shelf chips.
- [ADR 0415](0415-widgets-take-params-and-embed-where-pictures-draw.md): a pin
  creates an instance.
- [ADR 0432](0432-home-board-two-faces-three-sizes.md): the size contract, and
  who picks a new tile's size.
- [ADR 0466](0466-home-board-is-a-desktop-rail.md) point 5: "Add widget" pins
  to the shelf.
- [ADR 0496](0496-two-dashboards-with-their-own-tiles.md): the Home pin
  refusal.
- [ADR 0523](0523-widget-height-guard-and-restore-guard.md): the height guard's
  scope.

## Context

The maintainer reported that a widget worked in the thread but not at the
dashboard's small, medium and large sizes: "It doesnt show the whole thing."
The maintainer also found pin to thread did not work well. Demo Director takes showed both.
A widget whole in its thread card was cut off as a large desktop tile and as
every phone tile, and its text read small.

The cause was two size contracts for one document. In a thread the frame grew
to the widget's reported height, at the full transcript width. The knowhow said
"height follows the content, never 100vh". On a dashboard the host set a fixed
box per size and clipped the rest. The agent builds a widget in the thread, the
only place it sees it while making it. So it wrote for the first contract, and
the second one cut it.

No host code scaled a widget. The small text came from px type, which the UI
scale cannot reach. A workspace widget had even grown a `preview` param that
drew the tile box inline, to see its sizes.

The shelf was a second home for a widget inside one thread. The dashboards
(ADRs 0496, 0538) already are the place a widget stays in view, beside every
thread, on both layouts.

## Decision

1. **One widget box.** A widget with a size draws into a fixed box: `small`
   one square cell, `medium` two by one, `large` two by two. The host sets it.
   A content-height report never sizes it, and the widget never scrolls inside
   it. Content past the box is clipped under a fade.
2. **The thread card is a tile.** A widget in a reply draws as a bar over a
   widget box. The box is the one a dashboard tile draws, from the same
   component and the same cell edge. The bar holds the icon, the name, the
   S · M · L pill, "+ Dashboard" and the ⋯ menu.
3. **The card's size is the user's, per thread.** It starts at the widget's
   default size. A pick records `InlineWidgetSized`, and
   `widgets(action="size")` does the same from the prompt.
4. **Add to dashboard replaces pinning.** It adds the tile at the card's size,
   to the dashboard of the layout it is tapped in. `DashboardTileAdded` carries
   an optional `size`.
5. **Pin to thread is gone.** The shelf, its chips, `pin`, `unpin`, the thread
   widgets list, "Show in thread", the thread's "Add widget…" item and the SDK
   place `shelf` all go. `WidgetPinned` and `WidgetUnpinned` stay as legacy
   events the dashboard seed reads, and nothing writes them.
6. **Two places keep no size.** Canvas draws a widget as a full page. A question
   option draws it at its content height, so the height guard stays for options.

## Rationale

- **What the author sees is what ships.** The cut-off came from the author
  seeing one shape and the dashboard showing another. One box in both places
  makes "whole in the thread, cut on the dashboard" impossible by construction.
- **Fit by dropping content, never by shrinking it.** A host scale to fit would
  shrink text, which was half the bug. Type stays on the theme's rem tokens, and
  a smaller size shows less.
- **The viewport becomes the box everywhere.** `100vh` and container units mean
  the box in a card and in a tile. The inline contract's ban on them, and the
  height-report loop ADR 0523 guards against, leave the card.
- **One place to keep a widget in view.** A pinned chip and a dashboard tile did
  one job at two scopes. The dashboards are always reachable, synced, and in
  the agent's reach, so the thread-scoped copy goes.
- **Prompt-first.** The card size and Add at a size each have a manifest op.
  So the tool, the CLI and HTTP do what the pill and the button do.
- **A fold, not a migration.** Pins were events only. Old cards keep drawing
  from their embeds and `WidgetShown` rows, and Home's legacy pins keep seeding
  the dashboards.

## Consequences

- A widget written to the old inline contract now clips in its thread card at
  its default size, as its tile already did. The agent's guidance and the
  knowhow teach the box contract, so a rebuild fixes it.
- A plugin widget needs size layouts to look right in a thread, not only on a
  dashboard. No plugin manifest key changes.
- A pinned widget loses its chip. One that was pinned with no card in its
  thread leaves that thread's view. It is reusable, so "+ Add" still lists it.
  A release notice names Add to dashboard.
- On a desktop a large card is two cells wide in a wider column, so a thread
  shows a widget smaller than before. That is the dashboard's size, by design.
- The built-in sound and video players gain size layouts and declare the sizes
  they draw well. In an option they stay one row.
- `GET /api/v1/widgets/thread`, `/api/v1/widgets/pin`, `/api/v1/widgets/unpin`
  and their CLI verbs are removed. `/api/v1/widgets/size` is added.
- The agent still never chooses a size the user did not ask for. It passes one
  only when the user named it in that turn.

## Alternatives considered

- **Scale the frame to fit its tile.** One line of CSS, and every widget fits.
  Lost: text shrinks with the box, which is the other half of the bug.
- **Size the tile to its content.** Lost: the grid has three fixed sizes so an
  author designs three shapes (ADR 0432), and tiles of any height break it.
- **Keep content-fit cards, and only teach authors the tile rules.** Lost: the
  author still never sees the tile while making the widget, so the gap returns.
- **A preview of each size in the card's menu.** Smaller. Lost: two renders of
  one widget in a thread, and the default card still differs from the tile.
- **Keep pin, beside Add to dashboard.** Lost: the maintainer found pin did not
  work well, and the dashboards already keep a widget in view.
- **Convert each pin to a dashboard tile.** Lost: one-offs from every thread
  would flood a dashboard, and thread-owned widgets would turn reusable unasked.
- **Reset the card size on reload.** Smaller: no new event or op. Lost: a
  reload must keep what the user sees.
