# 0204: A live window the desk can no longer hold is refitted, and the trigger is the display configuration

- **Status**: Accepted
- **Date**: 2026-09-17

## Context

A tester on the packaged client reported that the window does not resize when
they switch monitors. Going from a big display to a small one, the density stays
put and content overflows or disappears. The maintainer reproduced it.

"Keep the same density" is what tells the two candidate faults apart. A
scale-factor conversion fault changes the font size, and this one does not. The
window geometry is wrong, and it is wrong while the client is running.

**ADR 0193 cannot reach this.** Every call site of
`window_restore::clamp_restored_geometry` is a window creation or a show:

- `open_app_window`, after the builder applied the frame.
- `settle_main_geometry`, from the startup show and from the tray reopen.
- `front_window`, for a window that is not up yet.

`sanitized_frame` is the same judgement one step earlier, and its one caller is
also a show. Nothing else in `crates/lucidos-app` reads the monitor set at all.

The one runtime display-aware handler is the `ScaleFactorChanged` arm, which
calls `app_window::refit_webview`. That puts the PAGE back over the whole
window, and never asks whether the WINDOW still fits a display.

So this report is the residual the ADR 0193 plan recorded at its end, arriving
as a user-visible bug: the code has no path that corrects a frame at runtime.

**What the platform actually delivers**, read out of the vendored `tao 0.35.3`:

- `ScaleFactorChanged` is emitted only from `windowDidChangeBackingProperties:`,
  and `emit_static_scale_factor_changed_event` returns early when the factor is
  unchanged. Undocking between two displays at the same factor fires nothing.
- `windowDidResize:` emits `Resized` then `Moved`. `windowDidMove:` emits
  `Moved`, and only when the origin actually changed. A resolution change under
  a window that does not move therefore emits no window event at all.
- Nothing in tao observes display reconfiguration.

## Decision

**A live window no attached SCREEN can hold is refitted, on the same rule ADR
0193 wrote for a restore.** `window_restore::fit_to_displays` is one gate in
front of `sanitize`, so the runtime path and the restore path cannot come to
different arrangements for one window.

**The gate is the size rule ALONE.** A frame no screen can hold is a shape no
gesture can produce. Everything else `sanitize` asks is about a rect nobody has
seen yet, and a live window's position is where the user put it.

**The trigger is `NSApplicationDidChangeScreenParametersNotification`, not a
window event.** That is AppKit's own "the desk changed" signal, and the one
event every variant of this bug shares.

**The pass waits for the desk to be still**, then runs once over every
non-fullscreen app window. `window_desk` owns the observer, the quiet period and
the walk; `window_restore` owns the judgement.

## Rationale

**The trigger is the only one that covers every variant.** No window event fires
for an undock at an unchanged scale factor, and none fires at all for a
resolution change under a still window. The screen-parameters notification fires
for all of them, because a display configuration change is exactly what it
reports.

**Two hazards are excluded structurally rather than tuned away.** A live drag
posts no screen-parameters change, so the pass cannot fire under the pointer.
Our own `set_position` and `set_size` post none either, so a correction cannot
cause another correction. Neither rests on a threshold anyone has to keep
calibrated.

**The lenient rule is what keeps the pass off the user's back.** A window that
still fits some attached display is a preference, and ADR 0193 already chose
that over the stricter reading. So the correction is reachable only by taking a
display away, which is not something the user does by accident.

**It waits because AppKit does not settle in one post.** The notification
arrives more than once per reconfiguration, and can arrive before AppKit has
relocated a window off a display that is gone. A pass taken then reads a
position AppKit is about to replace, which is the read-after-write trap ADR 0193
and ADR 0202 both paid for. A deadline each post pushes out coalesces the burst
and reads settled geometry.

**The size rule alone, because the other three rules are about a rect nobody has
seen.** A size under the declared minimum cannot be dragged into, and
`open_app_window` applies `min_inner_size` to every window it builds. A title
bar with nowhere to be grabbed is the user's arrangement, and ADR 0173 refused
to widen the clamp for one. Running the whole of `sanitize` at runtime would
move windows people put where they wanted them.

## Consequences

- The pass costs one poll thread and a mutex while the client is up.
  `watch_desk` spawns that thread only when the observer went in, so no platform
  without the notification polls anything.
- A correction is visible. The window sits at its old size for the quiet period
  plus one poll tick, then snaps. That is the price of reading geometry AppKit
  has finished writing, and the alternative is correcting against a position
  that is about to change.
- `clamp_restored_geometry` and `clamp_live_geometry` are now one body,
  `clamp_geometry`, differing in the question they pass it. The restore path's
  behaviour is unchanged.
- The lenient rule still leaves one shape uncorrected: a window sized for a big
  display, sitting on a small one while the big one is attached. That is ADR
  0193's deliberate choice, and it now has a row in `docs/known-gaps.md`.
- A fullscreen window is skipped, for the reason the restore clamp already gives.
- None of this is verifiable outside a packaged build (ADR 0016). The decision
  and the quiet rule are pure and covered; the seam between them is inspection
  plus the manual steps in
  `docs/plans/2026-09-17-a-live-window-is-refitted-when-the-desk-changes.md`.

## Alternatives considered

**Clamp on the first `Resized`.** ADR 0193 weighed and rejected this, and the
rejection stands. It fires on every user drag, so the clamp stops being a
one-shot and becomes a resize policy. It also does not fire at all for the
resolution-change variant, where the window never moves.

**Extend the `ScaleFactorChanged` arm.** The obvious place, and the wrong one.
tao suppresses that event when the factor is unchanged, so the arm is silent for
the same-scale undock the tester most likely hit. It would fix some desks and
not others, which is worse than not fixing it.

**Run the pass synchronously in the notification block.** Simpler, and it gets
the size right, since the new display set is published by the time the
notification posts. It gets the POSITION wrong whenever AppKit has not relocated
the window yet: the correction then centres it on the primary rather than
keeping the user's neighbourhood. It also runs once per post rather than once
per reconfiguration.

**Correct whenever the frame does not fit the display it sits on.** The stricter
rule ADR 0193 refused. It would catch a window dragged onto a smaller second
display, which is arguably part of "switch monitors". It also shrinks a window
whose big monitor is merely away, which the user would undo by hand every time,
and it would fire mid-drag.

**Convert `.window-state.json` to logical points instead.** The root fix for the
restore-time family, and ADR 0173 already weighed it. It says nothing about this
bug: no file is read here, and the window is live.
