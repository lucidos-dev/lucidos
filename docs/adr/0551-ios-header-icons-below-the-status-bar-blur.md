# 0551: The iOS home-screen app runs under a translucent status bar with no top strip, and the header's icons sit below iOS 27's blur, superseding 0549

- **Status**: Superseded by [0552](0552-ios-top-bar-follows-dynamic-bars.md)
- **Date**: 2026-10-22

## Context

ADR 0549 kept `black-translucent` and added a fixed 12px strip at the top edge
to keep iOS 27's blur off the header. On a fresh install the header's icons sat
one status bar height too low, under a solid band.

The safe-area geometry log showed why. The screen is 852px tall, but the page
was 793px tall with a top inset of 0. Once it started at 852px, then shrank. So
on iOS 27 the strip makes iOS paint an opaque bar and move the page below it.
The safe-area floor then read the 0 as a lost inset and added 59px back.

## Decision

The installed iOS app declares `black-translucent` and has no top strip. The
mobile header's fill starts at the top edge, so its colour runs behind the
status bar and the island. Its controls start at `--header-controls-top`: the
top inset plus `--status-bar-blur-clearance`, which only that app sets.

## Rationale

The user wants the header behind the island, and the icons must never blur.
iOS 27 offers no way to get both from the bar itself: a solid box at the top
edge removes the blur by making the bar opaque. The blur is iOS's own layer
above the web view, so the page cannot paint over it. ADR 0548 measured it
ending about 40pt below the status bar. Moving the controls below it is the one
shape that meets both wishes.

The safe-area floor also keys its memory by page height now. A page below an
opaque bar then never borrows the inset of a page behind a translucent one.

## Consequences

- The header grows by the clearance in the installed iOS app only. The header
  height anchors read `--header-controls-top`, and the measured
  `--mobile-header-height` includes it.
- Every other control fixed at the top edge starts at `--header-controls-top`
  too: the fullscreen exit, the file preview's header, the toast stack and
  the scroll-to-top chevron. So does the keyboard spacer in
  `useHideOnScroll.ts`. Scrolling content may still pass under the blur.
- The band between the status bar and the icons is header colour, blurred.
  Nothing readable sits there.
- The clearance is tunable per device while the user settles it
  (`docs/temporary-measures.md`, "Header blur clearance slider").
- The gateway splash no longer carries a strip.

## Alternatives considered

- **Keep the strip (ADR 0549).** It removes the blur, but on device it moves
  the page below an opaque bar. The header no longer reaches the island.
- **An opaque bar (ADR 0548).** Same result as the strip, declared outright.
  The user rejected it.
- **Translucent, and accept the blur.** The icons blur. The user rejected it.
