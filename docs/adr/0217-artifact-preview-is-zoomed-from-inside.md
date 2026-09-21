# 0217: An artifact preview is zoomed from inside the document, never on the iframe

- **Status**: Accepted
- **Date**: 2026-09-18

## Context

An HTML artifact is previewed in an `<iframe srcDoc>`. That document is its own
realm, so it inherits nothing from the shell. The host root font-size is
`var(--user-ui-scale)`; the artifact's is the browser's 16px default. At any UI
scale but 100% the artifact reads visibly smaller than the chrome framing it,
which is what a user reported.

## Decision

The preview stamps `:root { zoom: <scale>% }` into the artifact's own head,
beside the `<base href>` it already stamps (`withPreviewScale` in
`components/files/previewIframeLinks.ts`). It does NOT set `zoom` on the iframe
element.

## Rationale

The two engines we ship to disagree about `zoom` on an iframe element, and agree
exactly about `zoom` inside the document. Measured through Playwright, a 400x300
frame at zoom 1.25:

| | Chromium | WebKit |
|---|---|---|
| `zoom` on the **iframe element** | inner viewport 320px, content 1.25x, fits | inner viewport stays 400px, content painted 1.25x, so the right 20% is clipped |
| `zoom` on the artifact's **`:root`** | a 200px box paints 250px, `width:100%` still fits, no overflow | identical to Chromium |

The iframe-element form is the tidier one to reach for, since it needs no change
to the artifact at all. It is also the one that silently eats a fifth of every
report on Safari and on every iOS client. That is most of the phones a report
gets read on.

## Consequences

- The srcdoc carries the scale, so changing the UI scale re-stamps the document
  and the iframe reloads. A preview loses its scroll position when the slider
  moves, which is rare and cheap.
- 100% stamps nothing, so the default case is byte-identical to the artifact on
  disk.
- `100vw` and media queries inside the artifact still resolve against the
  UNZOOMED viewport, in both engines. An artifact sized in viewport units
  therefore overflows horizontally by the scale factor. That is a scrollbar
  rather than broken content, and an authored report uses a `max-width` column.
- The stamp is a plain `:root` rule at the top of the head. An artifact that
  genuinely wants to control its own zoom can still override it.

## Alternatives considered

- **`zoom` on the iframe element.** Rejected on the measurement above: correct in
  Chromium, clipping in WebKit. Compensating with a reciprocal
  `width: calc(100% / <scale>)` fixes WebKit and breaks Chromium, since the two
  resolve the inner viewport differently. A per-engine branch for a cosmetic
  scale is not worth owning.
- **`transform: scale()` on the iframe.** Needs the same reciprocal sizing, and
  additionally breaks scrolling and `position: fixed` inside the frame.
- **Setting the root font-size instead of `zoom`.** Reaches only `rem`-sized
  content. An artifact is written in px, so most of it would not move.
- **Applying the zoom from the host to the live `contentDocument` on load.** No
  reload and no scroll loss, but the frame paints once before `load` fires, so
  the reader sees the document re-scale. The srcdoc stamp is correct on the first
  frame.
