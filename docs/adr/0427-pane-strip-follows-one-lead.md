# 0427: The phone's pane strip follows one lead at a time, and only a lift commits

- **Status**: Accepted
- **Date**: 2026-10-11

## Context

Three sources move the phone's pane strip: host touches on it, drags an app
frame or the HTML preview posts up, and the header's own swipe. The state lived
in refs, a `SwipeTouch`, a frame-drag record and the track's inline style, and
nothing tied them together. The user reported intermittent odd states when
swiping. Recon found eleven reachable defects
(`docs/plans/2026-10-11-a-pane-swipe-reaches-no-invalid-state.md`):

- a cancel could switch panes;
- a second finger could hijack or block a swipe;
- a lost release could strand the strip mid-way;
- a navigation mid-drag left every later move animating.

## Decision

One pure state machine, `PaneDrag` (`components/layout/paneDrag.ts`), decides
who leads the strip. The lead is one host touch, followed by its identifier, or
one frame. Everything else is ignored until the lead ends. Only the lead's lift
commits: its `touchend`, or the lead frame's release. Everything else that ends
a drag springs back.

The rules that follow from it:

- **A new host touch replaces a lead that has not locked sideways.** A lead
  that has locked keeps the strip. So a thumb resting on the edge never hijacks
  a swipe.
- **A lead the browser no longer lists has ended.** Each touch event's own
  `touches` list is the platform's truth, so a lost `touchend` springs back.
- **An uncancelable move ends a host drag**, as it already did in the SDK.
- **A navigation, a resize or a fullscreen change ends the drag.** The strip
  slides to its pane and the rest of that gesture does nothing. An interrupted
  frame stays ignored until its own release, which the SDK always sends.
- **A frame whose document goes away says so.** `AppFrame` and the HTML preview
  send a host-side `gone` through `frame-swipe-bridge.ts`. No timer, and no
  change to the frame protocol.
- **Only a frame inside the strip may lead it**, so a floating widget window
  never swipes the panes behind it.
- **A drag grabs the strip where it is on screen**, at the lock. A tap or a
  vertical scroll never touches a running slide.

## Rationale

Every defect was two sources disagreeing about who owned the strip, or a drag
whose end nobody owned. One owner removes the first class. Making every ending
explicit removes the second: each path that ends a drag calls `rest`, and only
a lift may call `navigate` before it. The machine is DOM-free, so a seeded
random event sequence can check the invariants after every step, which no
amount of on-device swiping does.

## Consequences

- One adapter in `MobileSwipeContainer` is the only writer of the strip's
  transform and transition, besides the `transitionend` net.
- The header swipe runs the same machine with a track that draws nothing.
- The SDK is unchanged. Its tracker already springs back on a cancel and sends
  one release per drag.
- A host touch that has not locked gives way to a frame drag, and to a newer
  touch. Until one locks, a two-finger gesture in the host follows its newest
  finger.

## Alternatives considered

- **Patch each defect where it showed.** Rejected: the defects shared one
  cause. Per-site patches had already piled up, such as a touchstart that
  cleared the frame drag and a fresh gate per frame. Each one moved the drift
  rather than removing it.
- **A timer that ends a frame drag with no message.** Rejected: any timeout
  short enough to rescue a stranded strip also cuts a slow drag. The host
  knows exactly when a frame's document goes away, so it says so.
- **Follow the oldest touch (`touches[0]`).** That is what the code did, and
  it is how a resting thumb hijacked the swipe and cancelled every scroll.
- **Let the first touch lead until it lifts, whatever it does.** Rejected: an
  idle thumb would then block every swipe. Locking sideways is what claims the
  strip.
- **Ignore a host touch while a frame leads.** Rejected: a host touch means the
  finger is not in the frame, so the frame's release was lost. Taking over is
  what repairs a stuck strip.
