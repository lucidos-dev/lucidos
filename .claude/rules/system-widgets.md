---
paths:
  - "system-widgets/**"
---

# Built-in Widgets

`system-widgets/` holds the built-in widgets Lucidos ships (ADR 0415). Each one
is a folder named by its `lucidos-` id.

## Icons

Every built-in widget has an icon at `assets/icon.svg`, named by the manifest's
`icon`. Built-ins are ours, so their icons match the host's own glyphs in
`crates/lucidos-app/src/components/shared/icons.tsx`, not the bold full-colour
motif `system-knowhow/building-an-app.md` asks of a workspace app.

- **Draw on the host glyph grid.** The root `<svg>` sets `viewBox="0 0 24 24"`,
  `fill="none"`, `stroke-width="2"`, and round caps and joins. Keep about 3
  units of margin.
- **One outline motif.** No text, no gradient, no embedded image, no fine
  detail. It must still read at 18px.
- **One hue, shared by every built-in.** Copy it from an existing built-in
  icon. Lucidos draws the icon as an `<img>`, so it cannot follow the theme.
  The hue is a mid-tone blue that reads on dark and light tiles.
- **Only the main shape takes a tint**, `fill-opacity="0.2"` of the hue. A
  small solid mark, such as a play triangle, is filled solid.

`every_built_in_icon_follows_the_host_glyph_style` in
`crates/lucidos-engine/src/core/system_widgets.rs` checks the grid, the stroke,
the banned elements and the shared hue.
