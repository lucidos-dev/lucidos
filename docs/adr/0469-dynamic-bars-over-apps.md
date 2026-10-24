# 0469: Dynamic bars over an app: the SDK observes the app's scroll, the panel grows once

- **Status**: Accepted
- **Date**: 2026-10-13
- **Amended by**: [0482: The bars over an app wait for the finger to lift](0482-bars-over-an-app-wait-for-the-finger.md), which holds the header and the panel until the finger lifts

## Context

*Dynamic bars* glide the mobile header away on a scroll down (ADRs 0336,
0337). Over an app in the Canvas pane they never moved: `shouldKeepHeaderVisible`
pinned them. On a phone the app's frame fills the pane, and the frame's own
document scrolls. The host's scroll listener never fires, so the host cannot
see the reader's scroll at all.

An earlier attempt injected a script that consumed `wheel` and `touchmove` in
the frame, moved the header first, and only then let the frame scroll. It
double-scrolled and was reverted. The pin followed.

## Decision

The SDK observes the frame's scroll, passively, and posts the reader's delta to
the host (`lucidos:app:scroll`). The host folds it into the same intent the
transcript uses. While the header is away, the app panel glides up with it and
grows by its height at once. On the way back it shrinks only after the glide.

## Rationale

- **Observe, never consume.** The host bars already work this way: the scroll
  decides where the bars belong and a CSS transition moves them (ADR 0336).
  Nothing in the frame waits on the header, so nothing can double-scroll.
- **Only the reader moves the bars** (ADR 0337). An element's first scroll
  event sets its baseline, so the scroll-memory restore on load moves nothing.
  A scroll between a viewport `resize` and the next animation frame is the
  browser clamping, and re-bases with no delta. Without that rule, a loop
  follows at the bottom of a page. Growing the frame clamps its scroll, the
  clamp reads as a scroll up, and the bars return and shrink the frame.
- **Only the Canvas pane's frame counts.** The host checks the message source
  against the bound frame, as the pull and swipe bridges do. A widget in a
  thread, or an embed inside the app, cannot move the header.
- **The frame resizes once per transition, not per frame.** A per-frame resize
  of a cross-process frame is the expensive path on iOS. The panel grows as
  the glide starts and shrinks after the return. Its bottom stays on the
  pane's bottom throughout, at the cost of one `resize` each way.

## Consequences

- An app that loads the SDK gets dynamic bars with no code. An app that does
  not keeps its bars shown, as before.
- The find bar and dynamic bars off still pin the header.
- An element fixed to the bottom of an app slides off with the panel as the
  header returns, then snaps back when the panel shrinks.
- The header does not return near the end of an app, as the prompt does near
  the end of a thread. An app has no prompt.

## Alternatives considered

- **Keep the pin.** Simple, but the user asked for the bars over apps, and an
  app is often the longest page they scroll.
- **Consume the gesture in the frame** (the reverted attempt). It double-scrolled.
- **Shrink the header spacer per frame.** The frame resizes every frame of the
  glide, and its content moves with the spacer.
- **Extend the frame under the header and inset the app's content.** It needs
  every app to honour a host inset, which no existing app does.
