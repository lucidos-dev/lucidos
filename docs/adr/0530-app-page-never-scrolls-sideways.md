# 0530: An app page never scrolls sideways: sdk-iframe.css clips html and body on the x axis, no global min-width: 0

- **Status**: Accepted
- **Date**: 2026-10-19

## Context

An app on an iPhone scrolled sideways by about 1,700px. One history pill was
`flex: none; white-space: nowrap` and held free text, so it grew to the text's
width and dragged the whole page with it. Any app can make that mistake, and
the owner asked for page-level sideways scroll to be impossible in every app.

## Decision

The served `sdk-iframe.css` sets `overflow-x: clip` on both `<html>` and
`<body>`. `<html>` alone also carries `hidden` as the fallback. Body gets
`max-width: 100%`, and images and video get a zero-specificity
`max-width: 100%`. A too-wide child is cut off at the page edge. A deliberate
sideways scroller is an `overflow-x: auto` element of its own.

## Rationale

- **Both elements, measured.** With `clip` on `<html>` alone, the value moves
  to the viewport as `hidden`. A swipe then cannot move the page, but code can:
  `scrollTo` or a `focus()` on a wide input still shifts it, and `scrollWidth`
  still reports the wide content. With `clip` on body too, the overflow never
  reaches the viewport. WebKit and Chromium both report `scrollWidth` equal to
  the viewport, and `scrollTo(500, 0)` stays at 0.
- **`clip`, never `hidden`, on body.** With `<html>` non-visible, a `hidden`
  body becomes the page's scroller, which breaks window scrolling and sticky.
  `clip` makes no scroll container.
- **In the stylesheet, not the SDK.** Every app links the stylesheet, so
  existing apps get the guarantee with no edit and no script.

## Consequences

- Overflow is hidden, not fixed: a cut-off pill is still a bug in the app. The
  knowhow (`building-an-app.md`, "Responsive by default") says how to fix it.
- The packaged macOS webview treats `clip` like `hidden` (see the
  `.pane-header-brand` notes in `styles/panels/shell.css`). There the body
  becomes a scroll container that never scrolls, so `position: sticky` in an
  app does not stick in that build. Accepted by the owner.
- ADR 0420 saw the iOS edit menu fail under an `overflow: clip` ancestor of the
  host prompt box. That box sat on a translated, composited pane strip; an app
  body has no transform. No emulator shows the edit menu, so this is checked by
  hand on a device. If app text fields lose Paste or Select on iOS, revisit
  this ADR first.
- With `<html>` non-visible, body's `overflow` no longer moves to the viewport.
  So a scroll lock written as `body { overflow: hidden }` stops locking: body
  clips nothing and the page still scrolls behind a modal. An app locks scroll
  on `<html>` instead, which the knowhow says.
- The media cap covers images and video only. A percentage `max-width` lets a
  flex row or a table cell shrink a replaced element to nothing, which would
  squash SVG icons and charts.
- `e2e/app-no-horizontal-scroll-mobile.spec.ts` pins the guarantee, and a test
  in `api/sdk.rs` pins the rules in the served stylesheet.

## Alternatives considered

- **`<html>` only.** Lost: code can still shift the page, as measured above.
- **A global `min-width: 0` on every element.** Lost: the clip already stops
  page scroll. A global `min-width: 0` lets flex-shrink squeeze buttons and
  pills below their text, so the text spills out of its box instead. The
  author sets it on the one child that should shrink.
- **`contain: paint` on body.** Lost: it makes body the containing block for
  `position: fixed`, which moves every fixed toast and menu in an app.
- **An `app_lint` rule for `nowrap` plus `flex: none`.** Lost: too heuristic,
  since both are right on short fixed labels.
