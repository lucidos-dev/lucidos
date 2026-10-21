# 0538: The phone dashboard is a sheet over any thread, opened from a button in the title row; Home is a plain chat on a phone, as on a desktop

- **Status**: Accepted
- **Date**: 2026-10-20

Amends [ADR 0496](0496-two-dashboards-with-their-own-tiles.md) point 4 (the
phone keeps Home's two faces) and the two faces of
[ADR 0432](0432-home-board-two-faces-three-sizes.md) on a phone. The last line
of [ADR 0455](0455-desktop-home-chat-and-board-get-a-button-each.md), "the phone
is unchanged", no longer holds. Plan: `docs/plans/2026-10-20-phone-dashboard-sheet.md`.

## Context

On a phone the dashboard was one face of Home. So a phone saw the dashboard
only from Home, and a thread-aware widget such as `thread-cost` always
described Home. The maintainer asked to separate the two: the dashboard should
open over whatever thread is open, so a thread widget describes that thread.

ADR 0496 declined this split because it "would need a new way in". This
decision supplies the way in.

## Decision

1. **Home is a plain chat on a phone**, as on a desktop. The faces, the
   landing flag and the `.home-pane` markup are gone. The Home icon, the
   Lucidos menu's Home row and a first start all land in the Home chat.
2. **The *dashboard sheet* is the phone dashboard drawn over the open thread.**
   A button at the trailing end of the thread title row, on every thread, Home
   included, toggles it. The desktop keeps the *dashboard rail*.
3. **Opening the sheet never moves focus.** `focusedThreadId` stays the thread
   underneath. Any `focusThread` call, pane change or viewport change closes it.
4. **The sheet hangs from the title row.** Its top edge sits 0.75rem below the
   row's real bottom edge, with rounded top corners and a soft upward shadow.
   The header above the row and the strip of thread dim. The row and its button
   stay clear and tappable. A tap on the button or the dim closes the sheet.
5. **The motion rises from the bottom edge** and fades under reduced motion.
6. **`revealDashboard()` on a phone opens the sheet** over the current thread.
   That covers Add to dashboard and the SDK navigate target `dashboard`.

## Rationale

- **The dashboard and Home are different things.** A phone is the only layout
  that tied them, and the tie made thread-aware tiles answer for Home only.
- **A button in the title row is the way in ADR 0496 asked for.** It is on
  every thread, so the dashboard is one tap away wherever the user is.
- **Focus stays put**, so `lucidos.ui.focusedThread()` in a tile answers the
  thread under the sheet.
- **The sheet stays mounted after its first open**, hidden and inert. Moving
  or remounting an iframe reloads it, so a reopen finds every tile live.

## Consequences

- **The sheet does not use the shared `Overlay` panel.** `Overlay` unmounts its
  panel on close, which would reload the tile iframes. The sheet uses the same
  primitives directly: `pushOverlay` for Escape through the central stack and
  `useDismissOnOutside` for the outside tap. It applies no inert-behind, so the
  title row stays interactive.
- The sheet's open state is not persisted. A reload opens with it closed.
- No drag-to-dismiss and no swipe. The browser Back button does not close it.
- Home's empty state shows its welcome in the chat, as on a desktop, so
  `Dashboard` lost its `welcome` prop.
- `DashboardIcon` replaces `RightSidebarIcon` on the phone button and the rail
  toggle. A tablet keeps the rail unchanged.
- The two dashboards' data, the `widgets` tool, the CLI and the HTTP API are
  unchanged.

## Alternatives considered

- **Keep Home's two faces and add a thread dashboard beside them.** Lost: two
  ways to see the same phone dashboard, and the face still tied it to Home.
- **An anchored card under the header.** Lost: too small for a grid of tiles.
- **The button in the top header row.** Lost: that row is the app's, and the
  title row is where the thread's own actions already sit.
- **A bottom sheet with a grabber and half and full detents.** Lost: it adds a
  gesture and a second state for no gain over one full-height sheet.
- **A blurred backdrop.** Lost: a dim alone reads as clearly and costs less.
- **Floating tiles with no container.** Lost: tiles over a moving transcript
  lose their edges.
- **The sidebar icon on the phone.** Lost: it points at a sidebar a phone does
  not have. The tile icon names the dashboard on both layouts.

## Amendment, 2026-10-21: the button moves to the thread header

The maintainer reversed the "top header row" alternative above. The button now
sits in the thread header's centred cluster: Home, the Lucidos mark, then the
dashboard button. The mark stays on the row's axis. The title row keeps the
title and the widget shelf.

- **Why:** the dashboard is reachable without a focused thread, and the button
  no longer moves with a two-line title. Placing Home and the button either side
  of the mark also centres the mark, which the Home and mark pair did not.
- **Point 4 changes:** the sheet hangs 0.75rem below the app header, not the
  title row. Only the gap between the two dims, so the header and its button
  stay clear and tappable.
- Points 1 to 3, 5 and 6 are unchanged. Plan:
  `docs/plans/2026-10-21-dashboard-toggle-in-thread-header.md`.
