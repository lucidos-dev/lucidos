# 0361: Badges centre their glyph's ink, measured per font; WebKit keeps a half-device-pixel residual

- **Status**: Accepted
- **Date**: 2026-10-04

## Context

The mark's unread count drew its "3" visibly left of its pill on an iPhone at
200% ui-scale. The pill was centred. The font was not: `text-align: center`
centres a glyph's advance, and Fira Code draws its bold "3" 0.027em left of
that advance. Every font leans some digit this way. SF Pro's "1" leans 0.032em.

## Decision

Every glyph badge renders through `GlyphBadge`. It draws its own text on a
canvas in its computed font and scans the pixels for the ink's edges
(`drawnInkShift`). It states the gap as `--badge-ink-shift`, and `badges.css`
moves the text's own `.badge-ink` span by it, relatively, so only the paint
moves.

## Rationale

- **Measured, not tabled.** The user picks the font, and a workspace can ship
  its own. Only a measurement in the real font covers every digit in every font.
- **Pixels, not `TextMetrics`.** WebKit reports a glyph's ink bounds as its
  advance box, so `actualBoundingBoxLeft` is always 0 there.
- **A relative span, not an indent.** WebKit sized a content-sized pill by a
  `text-indent`, then drew its text half a pixel off it. A relatively positioned
  span changes no layout.

## Consequences

- Chromium centres the ink exactly at every ui-scale.
- WebKit moves glyphs in whole device-pixel steps from a fractional origin. A
  pill at a fractional device-pixel x can still sit its digit up to half a
  device pixel further off. The header's rem geometry puts the mark's count
  there at 200% (0.5px, against 0.83px before).
  `e2e/badge-glyph-centring.spec.ts` allows exactly that on WebKit, and still
  fails at the 0.67px the original bug showed.
- The mobile nav cluster is placed by `left`, not `translateX(-50%)`. A
  half-pixel translate added one more device pixel of the same split.

## Alternatives considered

- **One badge font (Geist Mono) whose digits all sit in their slot.** CSS-only,
  but badges would stop following the user's font. Offered and declined.
- **Script that snaps each badge to the device-pixel grid.** It would close the
  WebKit residual, but a badge moves on layout without any event to say so, so
  the snap would drift. Judged too fragile for half a device pixel.
- **Rounding the cluster's position.** Moved the badge itself onto a half pixel
  at other widths, and broke the mobile header alignment specs once already.
