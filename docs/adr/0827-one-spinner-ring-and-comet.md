# 0827: One spinner, a still ring with a comet turning on it

- **Status**: Accepted
- **Date**: 2026-11-27
- **Amended by**:
  - [0832](0832-spinner-solid-arc-not-comet.md): the turning layer is a solid arc, not a comet.

## Context

The shared spinner, `.mini-spinner`, looked wrong at most sizes and UI scales.
Three causes combined.

1. One absolute stroke served every size and was then snapped to whole pixels.
   At 125% it was 3px on a 10px drawer spinner.
2. Borders drew it. The arc ends were mitred points, and each side snapped to
   the pixel grid on its own.
3. The whole ring turned, so any unevenness wobbled.

Four places also drew their own spinner, at three speeds: Resuming sessions,
the activity badge, a waiting upload chip and the workspace picker's reload
button.

## Decision

There is one spinner. A faint ring stays still, and a comet turns on it once a
second. Both layers are gradients behind one ring mask, on the two
pseudo-elements of one empty `<span class="mini-spinner">`.

- A site sizes a spinner through `--spinner-size`, never `width` or `height`.
- The stroke is 8% of the diameter plus 0.0625rem.
- `--spinner-weight` is retired.
- A button that turns its own glyph is not a spinner. It shares the `spin`
  keyframes and the one-second turn.

## Rationale

A still ring hides unevenness, because nothing uneven moves. Only the comet
layer animates, and only `transform`, so it runs on the compositor and keeps
turning while the main thread is busy.

A gradient behind a mask has no sides, so no side snaps alone. The shared rule
snaps the box to whole pixels instead, and softens each edge over one device
pixel.

A stroke that follows the size is heavier in proportion when small and lighter
when large. One absolute weight cannot be right for a 0.5rem and a 0.875rem
spinner at once.

The markup contract stays one empty span, so every app gets the new look with
no change.

## Consequences

- An app that set `--spinner-weight` loses that control. The SDK setup knowhow
  says so.
- The edge ramp steps at 1, 1.5, 2 and 3 dppx. A screen at 1.25 dppx takes the
  1x ramp and draws slightly soft.
- The spinner needs `conic-gradient` and `mask`. `round()` keeps an `@supports`
  fallback, where the box is not snapped. The faint ring uses `opacity`, not
  `color-mix()`, so an older WebKit still draws it.
- Under forced colors both layers draw in `CanvasText`, whatever colour a site
  set.
- Two Vitest guards hold the shape: the spinner geometry, and the allowed users
  of `spin`.

## Alternatives considered

- **A breathing arc**, whose length grows and shrinks as it turns. It animates
  a paint property, so it repaints on the main thread and stutters when the app
  is busy.
- **An arc inside a ring.** Two concentric strokes turn to mush under 1rem,
  where most placements are.
- **An SVG component.** It draws the cleanest arc, but it breaks the one-span
  contract that apps rely on, and the engine-served stylesheet could not carry
  it.
- **Keeping borders with a proportional width.** Each side still snaps on its
  own, and the arc ends stay mitred.
