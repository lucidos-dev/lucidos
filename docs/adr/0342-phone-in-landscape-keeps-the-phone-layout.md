# 0342: A phone in landscape keeps the phone layout instead of the desktop split

- **Status**: Accepted
- **Date**: 2026-10-01

## Context

The layout line was width alone: 768px or less got the phone layout, anything
wider the desktop split. Most phones are wider than 768px when held sideways,
so rotating a phone swapped the whole layout. The reasoning was that wide
content (diffs, previews, images) suits the split.

In practice a landscape phone is about 390px tall. Two headers, the title bar
and the prompt left the transcript about 200px, and none of the phone
behaviour applied: no gliding bars, no keyboard handling, no notch insets on
the panes. The swap also left the dynamic bars bound to the discarded phone
layout after rotating back.

## Decision

A **phone in landscape**, a viewport at most 500px tall with a coarse primary
pointer, gets the phone layout. The line lives in one module,
`crates/lucidos-app/src/utils/layoutMedia.ts`. CSS writes `@media
(--phone-layout)` and `@media (--desktop-layout)`, which a PostCSS plugin in
`vite.config.ts` expands from that module, and `isMobile()` evaluates the same
numbers.

## Rationale

- **Height is what a phone lacks in landscape**, and the phone layout spends
  height best: one pane, and bars that glide away on scroll.
- **Rotating no longer swaps the layout.** Every piece of phone state survives
  it, so the class of bug where something stays bound to an unmounted layout
  cannot recur on rotation.
- **One definition.** The JS that mounts a layout and the CSS that styles it
  read the same numbers, so they cannot disagree about which layout is up.

## Consequences

- Tablets (at least 768px tall in landscape), short desktop windows and
  touchscreen laptops (fine primary pointer) keep the layout they had.
- A phone in landscape no longer shows two panes side by side.
- The phone layout must clear side insets, the Dynamic Island in landscape,
  which portrait never had.
- App iframes get `global/shared-components.css` raw, outside Vite, so the
  named queries cannot appear there. Its width-based rules are content-width
  choices and stay width-based.

## Alternatives considered

- **A compact desktop split for short screens.** Smaller headers and prompt
  still leave two narrow panes, and rotating still swaps the layout. The user
  chose the phone layout over it.
- **The full query written out in every stylesheet.** It would repeat a
  three-part desktop query about a dozen times, with nothing to keep the copies
  in step with the JS.
- **A root attribute set from JS, with CSS selecting on it.** It needs every
  rule in the media blocks rewritten, and the CSS waits on a script to run.
- **Level 4 `not` for the desktop query.** Shorter, but it relies on nested
  boolean media syntax. The plain Level 3 list works in every browser.
