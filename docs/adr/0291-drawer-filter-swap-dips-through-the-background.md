# 0291: The Threads/Filters swap dips through the background

- **Status**: Accepted
- **Date**: 2026-09-26
- **Partly supersedes**: 0287, for the drawer swap and both pane titles

*Amended 2026-09-26: the titles do not fade. The user asked for both pane
titles to switch word at once. So the title dip below is gone, and so is
0287's content title arrival. Only the views move.*

## Context

ADR 0287 made the Threads/Filters swap a page navigation. The leaving view went
in one frame, and the navigation cover cleared off the arriving one over
`--duration-normal` with `ease-out`.

On an iPhone the user still found it "quite instant", with "a little shake".
`ease-out` does most of a 200 ms fade in its first 60 to 80 ms. So the old view
vanished and the new one popped in: a blink. The headings of the two views sit
on almost the same lines, so the blink read as a jump. The title blinked the
same way.

## Decision

The swap dips through the pane background, the same both ways. The navigation
cover plays a `dip`: it rises over the leaving view, holds opaque, and clears
off the arriving one, over `--duration-slow` with `ease-in-out`. The CSS swaps
the two views at the midpoint by delaying their `visibility`, so the swap lands
under the opaque hold.

The pane title is a crossfade stack again, on the same timing
(`.crossfade-dip`). The old word fades out over the first 45%, and the new word
fades in over the last 45%.

The content pane keeps 0287's arrival cover and title arrival.

## Rationale

A transition the user can see needs both halves: the old view leaving and the
new one arriving. Both drawer views stay mounted, so the leaving one can fade
out. A content pane unmounts its leaving view, which is why it keeps the
arrival.

The dip keeps 0287's reasons for a cover. The content's own opacity never
moves, since WebKit re-compositing content up from transparent is the iOS
paint-loss shape. The cover is a keyed element replaying an animation, so it
never races a class toggle.

The swap sits in CSS, as a delayed `visibility` on the same token as the
cover's duration. So no timer can drift from the animation, and the Animation
speed slider scales both. The cover rests transparent and holds around the
midpoint. A frame or two of late start in WebKit then shows the leaving view,
and the swap still lands under an opaque cover.

## Consequences

- The two views are never on screen together, and each direction has the same
  shape.
- The swap takes 300 ms, not 200. Its middle is a brief flat background.
- A swap during a dip restarts the dip from its beginning. The view on screen
  then shows at full strength for a frame, where 0287 restarted from opaque.
- The drawer and the content pane now swap views differently. 0287's "one view
  swap looks like every other" no longer holds across panes.
- Reduced motion swaps both views and the title in one frame.

## Alternatives considered

**Retime the arrival cover: longer and `ease-in-out`.** The smallest change,
and one of the three offered. But the leaving view still vanishes in one frame,
so the blink stays.

**Slide the panel in from the right, like an iOS push.** Offered too. It reads
as a sub-page and moves differently from every other view swap in the app.

**Fade the views' own opacity.** The same look with no cover. It animates
content opacity on WebKit, which is the paint-loss shape 0287 avoided.

**Swap the views from a timer at the midpoint.** A timer can drift from the
CSS animation, and it needs its own slider scaling. A delayed `visibility`
transition cannot drift.
