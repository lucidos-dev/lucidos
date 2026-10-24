# 0496: Phone and desktop get their own dashboard, with their own tiles; Home's board face is the phone dashboard

- **Status**: Accepted
- **Date**: 2026-10-16

Amends [ADR 0432](0432-home-board-two-faces-three-sizes.md) (the tiles are
Home's pins, one arrangement per layout class) and
[ADR 0466](0466-home-board-is-a-desktop-rail.md) (the desktop rail).
Plan: `docs/plans/2026-10-16-two-dashboards.md`.

Amended by [ADR 0538](0538-phone-dashboard-is-a-sheet-over-any-thread.md): point
4 no longer holds. The phone dashboard is a sheet over any thread, and Home is
a plain chat.

## Context

The maintainer asked for two things. The phone and the desktop should show
different boards. And "Home board" should get another name. The desktop board
is a rail beside every thread (ADR 0466), so it has nothing to do with Home.

Until now one tile set fed both layouts: the widgets pinned to the Home thread.
Each layout class kept only its own order, sizes and hidden flags. A pin from
the desktop also landed on the phone, and the phone copied the desktop layout
until someone arranged it there.

## Decision

1. **There are two *dashboards*, `phone` and `desktop`, each with its own
   tiles.** Add to dashboard adds to the dashboard of the layout it is tapped
   in. The agent names one, and defaults to the last used device's.
2. **A dashboard is workspace state, not Home's.** Persisted system events
   record it: `DashboardTileAdded`, `DashboardTileRemoved` and
   `DashboardArranged`, aggregate `dashboard`. The Home thread takes no pins.
3. **Removing a tile is the hide.** A tile no longer has a hidden flag.
4. **The phone keeps Home's two faces**: the phone dashboard and the Home chat.
   The desktop dashboard is the rail, unchanged apart from its name.
5. **"Home board" becomes "dashboard" in every layer**, the SDK's widget place
   included (`home` becomes `dashboard`).

## Rationale

- **Different screens want different widgets**, not only different sizes. A
  hidden flag per layout made every pin a chore on the other screen.
- **The name follows the thing.** The desktop rail shows beside any thread, so
  calling it Home's misled both people and the agent, which routes on names.
- **Workspace state needs no carrier thread.** Pins on the Home thread tied the
  rail to Home in storage after the name stopped doing so.
- **A fold, not a migration.** Old events are immutable. The engine folds
  Home's legacy pins and `HomeBoardArranged` snapshots exactly as before, drops
  each layout's hidden tiles, and seeds each dashboard with the result.
  Dashboard events then apply on top. Nothing writes a legacy event any more,
  so the seed never moves.
- **The user agent picks the agent's default.** Nothing records a device's
  layout. An iPhone or an Android phone shows the phone layout; anything else
  shows the split layout, an iPad included. A turn with no device names one.

## Consequences

- A pin or unpin on the Home thread is refused, and the refusal names Add to
  dashboard. Unpin is refused too, since it would rewrite the seed.
- A dashboard tile names a reusable widget. Adding one that belongs to a thread
  makes it reusable first. Stop reusing is refused while a dashboard holds it.
- `GET /api/v1/home-board` becomes `GET /api/v1/dashboard`. The CLI's
  `widgets board` becomes `widgets dashboard`, beside `dashboard-add` and
  `dashboard-remove`. The navigate target `home-board` becomes `dashboard`.
- A widget that compares `data-widget-place` or `lucidos.widget.place()` to
  `home` stops matching on a dashboard. One attribute holds one value, so no
  alias can keep both. Sizing through `data-widget-size` is unaffected.
- The rail's width and shown state reset once, since their local keys moved.
- `HomeBoardArranged` stays a thread event the engine reads and never writes.

## Alternatives considered

- **Shared tiles, with the phone no longer copying the desktop.** Offered.
  Lost: a pin would still land on both screens.
- **Keep the tiles on the Home thread, with a dashboard field on each pin.**
  Smaller, but it keeps the desktop rail stored as Home's.
- **A one-time migration that writes the seed as dashboard events.** It needs
  a run-once marker and a recovery story. Folding the legacy events on read
  gives the same tiles with neither.
- **Record each device's layout for the agent's default.** A new column and a
  client report, for a guess the user agent already makes well enough.
- **Keep the SDK place `home`.** Offered as a fork at approval. Lost: one name
  per concept, and no widget in the tree reads the place.
- **Decouple the phone dashboard from Home too.** Asked. Lost: the phone's Home
  face works, and it would need a new way in.
