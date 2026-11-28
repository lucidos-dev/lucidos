# 0482: The bars over an app wait for the finger to lift

- **Status**: Accepted
- **Date**: 2026-10-14
- **Amends**: [0469: Dynamic bars over an app](0469-dynamic-bars-over-apps.md)

## Context

ADR 0469 let the mobile header glide away over an app. On an iPhone the result
was janky: the bars flickered back and forth through one slow scroll, while
the same scroll on a thread felt right.

The difference is the scroller. On a thread the header slides over a fixed
spacer, and the transcript never moves. Over an app the panel glides up with
the header, so the frame moves about 97px under the finger. iOS tracks a drag
in the scroll view's coordinates. A frame that moves up therefore reads as a
scroll up, which brings the bars back, which moves the frame again.

A Playwright frame trace measured it (plan
`docs/plans/2026-10-14-v0-49-0-release-blockers.md`, Part 3). Under that
finger model the header turned back 11 times in one slow drag and 20 times in a
brisk one. On a thread it turned back zero times.

## Decision

The host never moves the header or the app panel while a finger is on the app.
The SDK posts `lucidos:app:touch` with `{ touching }` from passive touch
listeners. The intent keeps following the scroll, and on the lift the bars take
the state it holds, with the same glide.

Two smaller changes ride along. The panel carries `will-change: translate`, as
every other bar does. The SDK knows the resize clamp by its shape (a move up
onto a bottom that came up), not only by its timing.

## Rationale

- **The scroller must not move under the reader.** Every jank in the trace
  follows from that one fact. Waiting for the lift removes it at the source.
- **Momentum moves no finger.** After the lift the frame can glide freely, and
  nothing reads the glide as a drag.
- **Touch events, not pointer events.** iOS cancels pointer events once a
  native scroll starts, so `pointerup` never arrives.
- **Observe, never consume** still holds. The touch listeners are passive and
  call nothing that changes the scroll (ADR 0469).

## Consequences

- Over an app, the bars move once the finger lifts: away after a scroll down,
  back after a scroll up. A thread still hides them during the drag.
- A reveal (a pane swipe, a new frame, a navigation) lets go of the finger, so
  a lost `touchend` cannot freeze the bars.
- The final check of the feel is manual on an iPhone. Playwright's
  `mobile-webkit` is desktop WebKit and cannot run a native drag.

## Alternatives considered

- **Extend the frame under the header, with an SDK-applied inset.** The
  scroller would never move. But many apps have a sticky or fixed bar, a
  full-height shell, or an inner scroller. On the dev workspace that is 13 of
  37 apps, and an automatic inset breaks each of them.
- **Scroll-link the panel to the header.** It replaces one glide with many
  small moves, and the scroller still moves under the finger.
- **Keep the pin** (bars always shown over an app). Simple, but the user asked
  for the bars over apps, and the hold-until-lift fix keeps them.
