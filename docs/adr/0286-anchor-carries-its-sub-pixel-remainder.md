# 0286: The scroll-anchor correction carries its sub-pixel remainder in a top spacer, so a turn-control press moves nothing

- **Status**: Accepted
- **Date**: 2026-09-25
- **Amends**: [ADR 0078](0078-anchor-correction-rounds-the-tween-does-not.md), its "residual is the floor" consequence

## Context

ADR 0078 rounds the anchor correction to a whole pixel, because neither engine
stores a fractional `scrollTop`. It accepted the leftover, up to half a CSS
pixel, as "the floor, not a defect".

It was not invisible. The macOS desktop app at 137.5% UI scale has a 22px root,
so almost every height is a fraction. Half a CSS pixel is one device pixel at
2x. Each line of text then re-lands on its own pixel row, and which lines jump
depends on their own fractional position. The user reported it as elements
moving slightly up and down on each steps or answer-only press, and different
elements on different turns.

## Decision

The correction still writes a whole pixel, now the one at or above the exact
offset. The remainder goes into `--anchor-subpixel`, a spacer in `[0, 1)` px
that both top reserves of `.thread-content` add: the desktop `padding-top` and
the mobile `::before` header spacer. Scroll and spacer cancel exactly.

## Rationale

**The floor was only the scroll offset's.** Layout lengths are fractional in
both engines (multiples of 1/64px). A spacer above every turn moves all of them
by any fraction, which is the one thing the scroll offset cannot do.

**Ceil, not round, keeps the spacer non-negative.** The spacer is
`ceil(exact) - exact`. A spacer in `[0, 1)` never shrinks a reserve and needs no
sign handling on mobile, where it adds to a height.

**It is not the debt ADR 0147 deleted.** Each press subtracts the current
spacer from its own two measurements. So an earlier press's spacer is part of
the reading, never a credit spent later. A press still moves only by its own
delta. The source scan in `toggle-holds-its-control-across-a-clamp.test.ts`
pins that shape.

## Consequences

- A press holds its control to layout precision. The browser spec
  `turn-control-holds-the-reader-still.spec.ts` bounds drift at 0.05px, at
  105%, 112.5% and 137.5%.
- The spacer persists between presses, at most 1px. Nothing else writes or
  reads it, and it is harmless when a transcript is reused for another thread.
- Every new top reserve on `.thread-content` must add the variable.
  `styles/__tests__/both-top-reserves-carry-the-anchor-rest.test.ts` checks the
  two that exist.
- A clamped press (ADR 0147) still slides, as before. The spacer cannot give
  the scroll range room it does not have.

## Alternatives considered

**Keep rounding and accept the residual.** ADR 0078's position. It is what the
user reported.

**Write the fraction and let the engine quantise.** ADR 0078 measured it: WebKit
truncates, so it is worse than rounding.

**A `transform` on the feed.** It moves content by a fraction too, but it has
three costs. It makes a containing block for fixed descendants and starts a
stacking context. It also breaks `contentOffsetTop`, which relies on nothing
between a turn and the transcript being transformed.

**Snap the layout to whole pixels.** Rejected in ADR 0078 already: it would
abandon rem-authored heights or the UI-scale setting.
