# 0532: An app joins a dashboard as a borderless quarter-size shortcut that opens it in the Canvas; no live app in a tile, no folder tile

- **Status**: Accepted
- **Date**: 2026-10-19

## Context

Only a widget could be a dashboard tile. `engine::widgets::widget()` refused an
app id, the "+ Add" picker listed only reusable widgets, and the Apps panel had
no "Add to dashboard". The user asked to add apps to a dashboard. They saw
rendered mockups in the real desktop rail, rejected a live app in a tile, and
asked for shortcuts: icon plus name, a quarter of a small cell. Between several
quarter tiles and one shortcut-group tile, they picked quarter tiles.

## Decision

An app joins a dashboard as an *app shortcut*. It is a quarter tile with no
border and no fill, and a tap opens the app in the Canvas. The grid steps by a
quarter cell. A small tile spans 2×2 steps, medium 4×2 and large 4×4. Every
widget box keeps its exact size.

## Rationale

- **No frame.** A live app in a tile mounts an iframe, and a frame costs a
  renderer process (ADR 0227). A shortcut mounts none.
- **Same events.** `DashboardTileAdded`, `DashboardTileRemoved` and
  `DashboardArranged` carry a shortcut as they carry a widget tile. The app
  manifest says whether a tile is a shortcut, read at read time. An app that
  changes kind changes its tile with no event, and old events read back
  unchanged.
- **One check.** `check_dashboard_change` and `check_arrangement` sit behind
  every surface (UI, `widgets` tool, CLI, HTTP). All accept an app id. All
  refuse params on one with the same message.
- **Impossible states.** A new `TileSize` (`quarter | small | medium | large`)
  sizes a tile. `WidgetSize` keeps three values and still sizes only widgets.
  The wire tile is a tagged union on `kind`, so an app tile with a widget's
  fields cannot exist.
- **Borderless.** The icon and name sit on the rail, as on a phone home
  screen. The user chose this over a bordered tile and over a border setting,
  so there is no stored border field. Edit mode outlines a shortcut like any
  tile.

## Consequences

- The grid has twice the resolution. A small tile is 2×2 steps.
- Tiles keep flowing in dashboard order (ADR 0419), so two quarters before a
  small tile can leave a gap. Edit-mode drag closes it.
- A widget may not declare `quarter`. `check_widget_manifest` refuses it.
- An app shortcut takes no params. A `label` renames it, through the tool,
  CLI and HTTP only.
- A tile whose app is deleted or unlisted drops off the view, as a widget tile
  does.

## Alternatives considered

- **A live app in a tile.** Rejected by the user. It costs an iframe per tile
  and an app drawn for a rail's width.
- **One shortcut-group (folder) tile.** Shown in the mockups and not picked.
  It hides shortcuts behind a second tap.
- **A bordered shortcut, or a border toggle.** Rejected by the user. It adds a
  stored field for a look the rail already gives.
- **A fourth `WidgetSize` value.** It would let a widget declare `quarter`. A
  separate `TileSize` keeps that state unrepresentable.
