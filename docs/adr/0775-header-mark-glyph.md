# 0775: The header mark is one flat glyph that signals connection by strength alone

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0237](0237-overlay-and-toast-paint-order.md)

## Context

`header-mark.css` styles the Lucidos mark in the header bar, its menu, and the
centred cluster that holds the nav chevrons.

The live style remote retunes a running app by writing custom properties inline
on `<html>`. Inline values beat a `:root` rule. The mark appears on several
header rows, and each row must place it the same way.

This record restates the design comments of
`crates/lucidos-app/src/styles/header-mark.css`, written before the comment
strip removes them. Nobody checked it line by line against the code.

## Decision

- Every quantity in the file is a custom property on `:root`. No magic number
  is written straight into a rule.
- The mark is one flat glyph on every row. It uses `--header-fg-muted` and
  brightens to `--header-fg` on hover and while the menu is open.
- Connection is shown as strength only. Connected holds `--header-fg`.
  Connecting breathes between two lower values. Disconnected sits still at the
  lower value.
- Colour is the only thing the connected rule carries. It adds no ring, tint or
  motion.
- The menu is a centred modal, portaled to `<body>`, and painted above
  `--z-toast`.
- The menu's inner radius is derived from the outer radius minus the frame
  inset.
- The centred cluster is absolutely centred on the row's middle, with a fixed
  width and the chevrons pushed to its edges.
- The cluster width is a clamp. It holds a span when it fits, gives way to the
  edge reserve when it does not, and never drops below the cluster's natural
  width.
- The cluster spans the row vertically instead of centring by its own height.
- The Workspaces row unfolds a list of satellite rows. They are inset, shorter,
  and at the value pill's size.
- The workspace restart row has no Cancel. Dismissing the menu is the cancel.
- Badges get no ring token here. `.app-header .badge` rings them for the whole
  bar.

## Rationale

- Strength keeps the whole connection signal in one dimension. The mark keeps
  the brand's colour rather than a status light's.
- A frame around the mark reads as an alert or a selection. Motion tells
  connecting from disconnected at a glance.
- Neither state is the whole signal alone. The `aria-label` in
  `BrandMenuButton` carries the rest.
- A centred modal lands in the same place whichever header opened it and
  whichever edge the mark sits on.
- A derived inner radius keeps the rows' corners concentric with the panel at
  any retune. A constant would go wrong when either value moves.
- The portal is structural. The mobile `.pane-header` carries `will-change:
  translate`, which makes it the containing block for `position: fixed`
  descendants. Offsets would resolve against it instead of the viewport.
- The menu sits above `--z-toast`, which is above `--z-modal`. A surface the
  user opened must paint over a persistent toast such as "Restart needed".
- Centring on the row's middle avoids drift when the two edge clusters differ
  in width.
- A fixed width puts the chevrons at the same two positions on the thread pane
  and the content pane. A cluster sized to its contents did not.
- A plain rem constant fails because the row has a fixed device width while
  every control is rem-sized. A span that clears the edges at 100% ui-scale
  overlaps them at 150%.
- Spanning the row vertically gives every row the same chevron placement.
  Centring by own height let each row inherit the rounding of its tallest
  member.
- The edge reserve is the widest edge cluster either pane can hold: the
  overflow trigger plus the bell. It is a slot held, so a chevron does not move
  when the trigger appears.
- The min span is the thread row's narrowest width. It engages past about 126%
  ui-scale on a 375px phone.
- The panel width is shared as `--header-surface-width`. The gap below the
  header is `--header-surface-gap`. Both live in `global/base.css`, because
  every header surface shares them.
- The Workspaces row is the widest row. The monospace advance is about 0.6em,
  so the room a name gets is arithmetic.
- The direction marker for the unfolding list takes the check's footprint in
  the pill. The width is a budget, and the unfolded list still marks the
  current workspace.
- Satellite rows keep the panel reading as a few items with a list hanging off
  one of them.
- Having no Cancel keeps the restart row inside the panel's fixed width instead
  of clipping against `overflow: hidden`.
- The badge ring fixes fusing that belongs to the bar, not the mark. The bell
  owns it too.

## Consequences

- Any retune of the mark, menu or cluster goes through custom properties and
  needs no rebuild.
- Any control added to either edge of the header spends the edge reserve.
- `e2e/mobile-threads-title-alignment-all.spec.ts` pins the cross-pane
  agreement and the clearance.
- The clamp states what `useHeaderActionCollapse` used to measure. It works
  only because the trailing cluster is now bounded, see `ContentHeaderActions`.
- Panel width changes affect every header palette that shares
  `--header-surface-width`.
- The mark's prominence comes from size and position, not paint.

## Alternatives considered

- A filled gradient tile for the mark. It read as a badge stuck onto the row,
  not as one of its controls.
- A muted mark in every state, with no connected rule.
- A ring, tint or hue on the connected mark. A frame reads as an alert or a
  selection.
- A panel hanging off the mark instead of a centred modal.
- A constant inner radius for the menu rows.
- Placing the menu at `--z-modal`. A persistent toast would paint over it.
- A panel width of 15rem. It left the name 7 characters and cut "development"
  to "develo…". 17.5rem held 13.
- A direction marker beside the value pill. It overspent the width by about a
  character and a half.
- A Cancel button beside the restart row.
- An in-flow cluster sized to its contents. It drifted off the viewport axis
  and moved the chevrons.
- A plain rem constant for the cluster width.
- Placing the cluster with `top: 50%` and `translateY(-50%)`.
