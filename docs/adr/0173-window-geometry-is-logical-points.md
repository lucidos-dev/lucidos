# 0173: The client's window geometry is stored and judged in logical points, because tao's physical pixels are not one space across mixed-DPI monitors

- **Status**: Accepted
- **Date**: 2026-09-05

## Context

A user opened a workspace from the picker while looking at another one. The new
window came up very wide, far down the screen, and mostly below the bottom edge.
They had never left it there.

Their desk has two displays at different scale factors: a 5120x1440 ultrawide at
1.0 as the primary, and the internal 3456x2234 Retina panel at 2.0.

The client stored and judged every window rect in what tao calls physical
pixels. `window_restore.rs` claimed that was "the only space shared by monitors
running different scale factors". On macOS that is false. macOS has one global
desktop coordinate space and it is measured in **points**. tao derives its
physical numbers from that space by multiplying, using the scale factor of
whichever object it is reading:

| tao call | multiplies the point space by |
|---|---|
| `Window::outer_position`, `Window::inner_size` | the WINDOW's scale factor |
| `MonitorHandle::position`, `MonitorHandle::size` | that MONITOR's scale factor |
| `set_outer_position`, `set_inner_size` with a physical input | divides by the window's scale factor |

Each display therefore gets its own projection of the desktop, and the
projections disagree. The reporting machine's own record held both at once. Most
windows sat at `1280,30 2560x1410`, half the ultrawide, where points are pixels.
One sat at `4050,3250 3456x2168`, the laptop panel maximized, with every number
doubled.

`app_window::place_window` handed the remembered rect to `set_size` and
`set_position` as physical pixels. tao divided by the scale factor of whichever
display the new window was born on. When that differed from the capture display,
the size and the position came out scaled by the ratio. Two to one is a window
twice as wide, twice as far down, and hanging off the bottom.

`window_restore::clamp_restored_geometry` could not catch it. It compared the
window rect against monitor work areas in the same broken space. A doubled rect
therefore looked at home on whichever projection it landed in.

## Decision

**Logical points are the one coordinate space the client remembers, judges and
applies window geometry in.** Every physical number converts the moment it is
read, through the scale factor of the object that reported it: the window's own
for a window, the monitor's own for a monitor. Placements and corrections go out
as `LogicalPosition` and `LogicalSize`.

The window session record (ADR 0123) carries a `units` marker. A record without
one predates this decision, so its frames are dropped rather than reinterpreted.

## Rationale

**Points are the space macOS actually has.** `CGDisplayBounds` returns each
display's origin in points, and the displays tile without overlapping. Physical
pixels are a per-object projection of that space, not a space of their own. A
projection is only meaningful next to the object it was taken from.

**One space beats one conversion.** The alternative was to keep physical storage
and convert at the point of use. That leaves every future reader asking which
scale factor a given rect belongs to, which is the question that produced the
bug. Converting at the boundary means nothing downstream can ask it.

**Logical placement also survives a display change mid-call.** `place_window`
sizes and then positions. Those two calls can straddle a move between displays,
and a physical value would then be divided by two different factors. tao passes
a logical value through untouched, so the pair cannot disagree.

**The clamp becomes sound as a side effect.** Its whole job is comparing a
window rect against monitor rects. In the old space that comparison was between
two different projections whenever the displays differed in scale.

**A dropped frame costs one launch, a converted one could cost trust.**
Recovering the capture scale means guessing which monitor's projection a legacy
rect came from. That guess is ambiguous exactly when two projections overlap,
which is the mixed-DPI case this fixes. `open` survives the migration, so the
windows still come back. Only their remembered sizes are lost, once.

## Consequences

- `window_restore::Rect` means points everywhere it appears: the record, the
  clamp, the restore plan, and every placement.
- `Policy` no longer multiplies by a scale factor. `tauri.conf.json` already
  declares points, so the declared minimum reaches the clamp unchanged.
- A scale factor the client cannot read now skips the work rather than falling
  back to 1.0. It converts the rect, so a guess is a wrong rect rather than a
  coarse threshold.
- Every user loses their remembered window sizes once, on the first launch after
  the upgrade. The windows themselves still reopen.
- `tauri-plugin-window-state` still stores physical pixels for `main`, and we do
  not control it. It stays because it is the only thing that remembers `main`'s
  frame while `main` sits on the picker, which the session record never keys.
  Its restore is superseded whenever the record holds a frame for the workspace
  `main` opens. What it leaves behind is now judged by a clamp that reasons in
  one coherent space. So the residual is a first-launch `main` at the wrong
  size, still placed somewhere reachable.

## Alternatives considered

**Keep physical storage and record the capture scale beside each frame.** It
works, and it would have preserved the existing frames through the upgrade. It
also keeps a rect whose meaning depends on a second field, so every reader has
to carry the pair. The bug was a reader that did not, and this makes that class
of reader possible again.

**Convert legacy frames instead of dropping them.** A physical rect can be
inverted by finding the monitor whose physical rect contains it and dividing by
that monitor's scale. That inverse is correct when the projections are disjoint
and ambiguous when they overlap. Overlap is precisely the mixed-DPI case, so the
heuristic fails hardest on the users this fixes, and it would run on the boot
path.

**Take `SIZE` and `POSITION` away from the window-state plugin.** That would
leave no physical-pixel store anywhere, which is tempting. It also loses `main`'s
frame whenever `main` is on the picker, because the session record is keyed by
workspace and the picker is not one. A remembered picker window is worth more
than the tidiness.

**Tighten the clamp so a window mostly below the screen is pulled back.** The
reported window did hang off the bottom, so this looks like a fix. It is not:
the window was wrong because its numbers were doubled, and a user may
deliberately park a window with only its title bar showing. Widening the clamp
would move windows people put where they wanted them.
