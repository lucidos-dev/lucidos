# 0244: A zoomed window re-fitting across displays is AppKit's, not ours

- **Status**: Accepted
- **Date**: 2026-09-22

## Context

A user reported that the client's windows change size when dragged to the other
monitor, and that their Chrome windows do not. The desk is the one ADRs 0173 and
0178 were reported on. It has a 5120x1440 ultrawide at scale factor 1.0 as the
primary, and an internal 3456x2234 Retina panel at 2.0.

Six ADRs already cover the client's window geometry: 0123, 0173, 0178, 0193,
0202 and 0215. So the report read as a seventh, and the suspects were the
restore clamp, the desk pass and the webview refit.

None of them fired. The client logs every correction it makes, through
`window_restore::log_correction`, and no line was written at any of the moments
the window changed size.

Sampling the live frames four times a second settled it. Two facts came out:

- A window at a hand-set size crossed both ways unchanged. One held 1029x1271
  and another held 1159x1084, on both displays.
- A window the user had filled with the green button flipped between two sizes,
  one per display, on every crossing. A ten-point drag of its edge cleared the
  state for good.

The control was the same programmatic zoom and cross-display move, run against
Finder and against the client:

| window | filled on the ultrawide | moved to the laptop |
|---|---|---|
| Finder | 906x1410 | 906x1084 |
| the client | 5120x1410 | 5120x1084 |

Both lose exactly the difference between the two work-area heights, 1410 points
against 1084. A plain AppKit window behaves identically, so the behaviour is the
platform's.

## Decision

**A zoomed window's size across a display change is AppKit's to decide, and the
client does not correct it.** `window_restore::fit_to_displays` keeps its one
gate, the size rule, and gains nothing for the zoomed case.

## Rationale

**A zoomed window is one the user asked the system to fit to a screen.** Filling
the display it is on is what that request means. Carrying the old screen's size
onto the new one would be the wrong answer, not a preserved preference.

**Every app does it.** The Finder row above is the whole argument. Fighting it
would make the client the odd one out on the user's desk.

**The clamp already refuses this class of work.** ADR 0173 declined to move a
window a user had parked on an edge, and ADR 0215 keeps a correction out of the
record. A zoomed frame is the system's arrangement, which is the same rule seen
from the other side.

**The exit is already in the user's hands.** One drag of an edge un-zooms the
window, and it then keeps its size across both displays.

## Consequences

- A report of this shape is closed by asking whether the window was filled with
  the green button, before any code is read.
- `clamp_geometry` logging every correction is what made the diagnosis possible.
  Keep it: a silent correction path would have cost far more here.
- The client reaches the zoomed state easily. `toggle_window_maximize` binds a
  double-click on the reclaimed title-bar strip to `zoom:`, and
  `window_state_flags` restores `MAXIMIZED`. Both match AppKit, so neither
  changes.

## Alternatives considered

**Remember the size and re-apply it after a display change.** It answers the
report literally. It also overrides a zoom the user asked for, and it would run
on a window AppKit is still animating. ADR 0178 records what a placement issued
against a moving window costs.

**Drop `StateFlags::MAXIMIZED`, so a window never returns zoomed.** That would
reduce how often the state is met. It also throws away a real preference, and
the state is still one double-click away.

**Honour the system's double-click preference (`AppleActionOnDoubleClick`)
rather than always zooming.** Correct on its own terms, and unrelated to this
report. That preference decides what a double-click does, not what a zoomed
window does across displays. Left as a separate question.
