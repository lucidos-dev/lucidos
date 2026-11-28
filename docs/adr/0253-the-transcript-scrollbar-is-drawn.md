# 0253: The transcript scrollbar is drawn, because it measures the thread and a native one measures the DOM

- **Status**: Superseded by [0258](0258-the-transcript-scrollbar-measures-the-drawn-slice.md)
- **Date**: 2026-09-23

## Context

The transcript draws a *render window* of a paged thread (ADR 0230). A native
scrollbar measures the DOM, so it reports position in the drawn slice only.

Reported on desktop: coming back to a long thread, the thumb sat near the top
of its track while the reader was deep in the thread. The up chevron, which asks
the thread, correctly said there was more above. Every window grow and every page
load then made the native thumb jump and shrink.

Mobile already drew its own indicator, for a layout reason (the fixed header
covers the scroller's top). Its plan left desktop on the native scrollbar,
because on desktop the native one is aligned. It was aligned, and still wrong.

## Decision

The transcript scrollbar is drawn in both layouts. Its range is the drawn slice
extended by an estimate of the undrawn history, counted in events. The server
reports how many events sit behind each page (`olderCount`), so a page landing
leaves the estimate unchanged.

On desktop it is a control in the native gutter: drag, track press, wheel. On
mobile it stays a touch indicator.

## Rationale

The thumb has to answer the question the reader asks of it: where am I in this
thread. Only the client knows how much is undrawn, and only the server knows how
much is unloaded, so the thumb needs both counts.

Events are the unit because both sides can count them. A per-turn average is
wrong on a coding-agent thread, where one turn can hold hundreds of steps. And
the loaded turn count moves on every page load, which was the mobile jank.

The native gutter stays reserved, so nothing about the composer alignment
(`--scrollbar-gutter-width`) changes. Only the native thumb goes transparent.

## Consequences

- The thumb drifts on a window grow by the estimate's error for the turns drawn,
  never by their full height. It is exact once the whole thread is drawn.
- A drag above the drawn slice parks at the slice's top. The existing growth
  draws older turns from there, and the thumb waits for the content rather than
  running ahead of it.
- Our control must reproduce what the native one gave for free. A drag and a
  track press stamp a scrollbar gesture, and a wheel is forwarded to the
  transcript. So the *standing follow* and the scroll-up growth see them as
  before.
- It is gated on a fine pointer and on `::-webkit-scrollbar` support. A browser
  outside that gate keeps the native scrollbar, and its old inaccuracy.
- The events endpoint does one more count per page, off the existing
  `(thread_id, created, sequence)` index.

> **Amended: the thumb moves only when the reader scrolls.** Drawn straight
> from the estimate, it jumped on the first scroll after an open, which usually
> draws older turns. It now holds the head it is drawn with (`nextThumb`).
> Content drawn above with an anchor write gives up exactly that height, and
> content below leaves it alone. An upward scroll walks the head back to the
> estimate by the thread's first line, never against the scroll. A drag maps
> against the thumb as drawn.

## Alternatives considered

**A spacer for the undrawn history.** Give the scroller a real range by putting
an element of the estimated height above the window. The native scrollbar would
then be right with no drawn control. Rejected: every transcript reader of
`scrollTop` assumes the scroller's top is the drawn slice's top. That is the
fill, the scroll-up growth, the anchor correction, the reading position and the
chevrons. The spacer would change that contract for all of them to fix one bar.

**Estimate per turn, client side only.** No engine change. Rejected: the loaded
turn count grows with every page, so the thumb still jumps when a page lands,
which was half the report.

**Hide the native scrollbar and draw ours without a gutter.** Simpler CSS.
Rejected: the gutter width is published to the composer and every surface that
lines up with the transcript. Dropping it moves all of them.
