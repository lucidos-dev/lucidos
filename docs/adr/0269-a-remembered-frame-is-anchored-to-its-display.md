# 0269: A remembered window frame is anchored to its display, and an orphaned frame is not recorded

- **Status**: Accepted
- **Date**: 2026-09-24

## Context

A user updated the DMG, and a workspace window moved from the built-in
display to the external one. It was the third report of that symptom. ADR 0193,
ADR 0204 and ADR 0215 each fixed a real defect on the way, and none of them
fixed this one.

The desk behind it is an ordinary laptop setup:

| Display | Frame (top-left, logical points) |
|---|---|
| External, primary | 0,0 5120x1440 |
| Built-in, below it | 1763,1440 1728x1117 |

**macOS measures every window from the primary display's corner.** Undock, and
the built-in becomes the primary at 0,0. So the whole coordinate space shifts by
(1763, 1440) at every dock and every undock. A built-in window at 0,33
undocked is the SAME place as 1763,1473 docked.

**The session record stored raw global coordinates.** A frame captured on one
side of a dock and restored on the other lands 1763 points left and 1440 points
up. For a built-in window that is the external display. It is a healthy frame
there, so no clamp fires and nothing is logged. One of the reported windows sat
at 0,356 on the external: exactly 0,33 on an undocked built-in, replayed docked.

**An unplug also leaves orphaned frames behind.** When a display goes, macOS
keeps an orphaned window's AppKit coordinates for a while. The client log shows
it: every external window at y=30 read back at y=-293, which is 30 minus the
difference between the two displays' heights. The same arithmetic turned an
external frame at 643,132 into 643,-191.

That frame fits the built-in's SIZE, so the live pass rightly left it alone
(ADR 0204), and the flush recorded it. Every later launch restored it above
every screen, and the rescue put it on the primary. ADR 0215's hold then kept
it in the record, so the move repeated.

## Decision

**Each remembered frame carries a display anchor**: the identity of the display
it was on, and that display's frame at capture time. A restore shifts the frame
by however far that display has moved since. With no single matching display,
the raw frame is restored exactly as before.

**A frame whose title bar is on no attached screen is not recorded.** The record
keeps what it held for that workspace, the same rule ADR 0215 applies to a
rescue.

## Rationale

**A position belongs to a display, not to the desktop.** The user put a window
on the built-in, not at a coordinate. The display is the thing that survives a
change of primary, a rearrangement and a relaunch, so it is what the record
should name.

**The identity is tao's monitor name plus the logical size.** On macOS tao names
a monitor `Monitor #<CGDisplayModelNumber>`, which is stable across launches and
arrangements. The size tells two identical models at different resolutions
apart. Two identical monitors at one resolution are ambiguous, and an ambiguous
anchor is ignored rather than guessed at.

**The orphan rule is the loosest one a gesture can satisfy.** macOS will not let
a drag put the title bar off every screen, so a frame like that was put there by
the system. The test asks only that SOME of the title strip lies on SOME screen.
A window the user parked almost off an edge still passes, which is the case ADR
0173 and ADR 0204 refused to break.

**Both rules fail toward the old behaviour.** An unmatched anchor restores the
raw frame, and the existing clamp judges it. An orphan with nothing held is
recorded, because a workspace with no frame at all is worse.

## Consequences

- A window comes back on the display it was left on, whichever display is the
  primary at restore time.
- The record gains an optional `display` key beside each frame. An older build
  ignores it and reads the frame as before, so a rollback loses nothing.
- A record with no anchor restores exactly as it did. Nothing moves until the
  next capture anchors each frame.
- The clamps decide exactly what they did. They now judge the shifted frame.
- Each shift is logged with the display and both frames.
- Off macOS the monitor name is whatever the platform reports. A mismatch only
  means the anchor is ignored.
- The seam needs a packaged build to check by hand (ADR 0016). The anchor, the
  shift and the orphan rule are pure and unit-tested.

## Alternatives considered

**Wait for the desk to settle before recording.** A quiet period after a
display change. ADR 0204 and ADR 0215 both refused a tuned threshold, and it
fixes neither half here: a frame recorded undocked is correct when it is
recorded. It is the replay in another space that is wrong.

**Record the primary display's identity once, and drop frames from another
primary.** It throws away every frame after a dock, so each window comes back
at the default. The anchor keeps them.

**Store the frame relative to its display, instead of beside the global one.**
Cleaner on paper, and an older build would then read offsets as global
coordinates and put every window near the primary's corner. Keeping the global
frame and adding the anchor costs one subtraction.

**Bump the record's units marker.** It would drop every remembered frame once
for no gain, since an unanchored frame already means "restore as before".

**Run the position rule in the live desk pass too.** It would rescue an orphan
on screen, and it would also move windows the user parked on an edge, which
ADR 0204 rejected. Not recording the orphan is enough to stop the move at the
next launch.
