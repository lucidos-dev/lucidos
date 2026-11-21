# 0215: A frame the client corrected is not written back as the user's arrangement

- **Status**: Accepted
- **Date**: 2026-09-18

## Context

A user relaunched the packaged client and one window came back on the external
display instead of the built-in one it had been left on. The client log names
the move:

```text
Restored window geometry 1728x1084 at 643,-191 is unusable on the attached
displays: correcting to 1728x1084 at 643,30 (logical points)
```

That is the restore clamp doing its job. The remembered frame sat above every
work area the client could see. ADR 0193's position rule nudged it down onto
the display that was there. What the user reported is not the nudge. It is that
the window never went back.

**The record absorbs the correction within a second.** `window_persist`'s
debounced flush reads LIVE geometry, and a placement fires `Moved` and
`Resized`, which arm the flush. So the rescue is written into
`.window-session.json` as the frame that workspace is remembered at. The
arrangement the user actually chose is then gone, and no later launch can
restore it however complete the desk is.

The clamp has no way to be sure the desk is complete. Two shapes produce a
frame no attached work area holds, and they are indistinguishable at the moment
of the read:

- The display really is gone, and the window is genuinely stranded.
- The display is attached but not yet published, which a relaunch and a
  reconfiguration both make reachable.

ADR 0204 already records that AppKit does not settle in one post, and paid for
the same class of read once.

## Decision

**A frame the CLIENT chose for a window is not recorded as the arrangement.**
While a window is still wearing a rect the clamp placed, the session record
keeps the frame it already held for that workspace.

**"Still wearing it" is the rect itself.** Each correction site records the
exact rect it placed, by window label. The capture asks whether the window's
live frame is still that rect. Any other frame means the user moved or resized
the window, so the note is dropped and their frame is recorded from then on.

**A workspace with no remembered frame records the correction.** There is
nothing better to keep. A workspace with no frame at all reopens cascaded from
the window that asked for it. With no such window, it takes most of the
primary's work area (`window_restore::new_window_frame`).

**The window-state plugin is stood down rather than corrected.** It covers
`main` alone, and reads live geometry itself. Not calling it is therefore the
only way to keep a rescue out of its file.

## Rationale

**The user's arrangement is the durable fact, and a rescue is a repair.** The
clamp exists so a window is reachable right now. Nothing about it says the user
wants the window there, and ADR 0193's own framing is that a position is where
the user put it. Recording a repair promotes a guess about today's desk into an
answer about every future one.

**The frame comparison needs no clock and no new event.** tao reports `Moved`
and `Resized` without saying who caused them, so a handler cannot tell our own
placement from a drag. Comparing against the rect we asked for sidesteps that
entirely: the rect is the only thing both sides agree on, and the first gesture
that changes it is the signal.

**It fails toward the old behaviour.** If macOS lands the window a point off
what was asked, the frames differ, the note is dropped and the live frame is
recorded. That is exactly what shipped before this decision. Both sides of the
comparison are logical points and macOS backs a window at 1x or 2x, so the
round trip is exact in practice.

**Holding a frame can only cost one relaunch.** The worst case is a record that
keeps a frame the desk cannot serve, which the clamp then rescues again. The
opposite failure, recording the rescue, destroys the arrangement permanently.

## Consequences

- A window rescued because a display was away returns to it when the display
  comes back, with no action from the user.
- A workspace whose window is left untouched after a rescue keeps its old frame
  in the record. So the file can name a frame no current display can hold, and
  that is deliberate.
- `main`'s maximized and fullscreen flags are not recorded while it wears a
  rescue. Both change the frame, so the suppression ends the moment either does.
- **The plugin's own exit hook is not intercepted, deliberately.**
  `tauri-plugin-window-state` saves on `RunEvent::Exit`, so Quit writes `main`'s
  rescued frame into its file whatever the client does. It costs nothing. That
  file is read only for a workspace the session record has no frame for. There
  the session record takes the correction too, so the two agree.
- The note is process-local and dies with the client, which is right: it
  describes a window on screen, not a fact about the record.
- A window that is destroyed drops its note in the `Destroyed` arm. `main` is
  hidden rather than closed, and comes back on its workspace. A note outliving
  its window would hold the record against a later, unrelated frame.
- Nothing is added to `.window-session.json`, so an older build reads every
  record this one writes. ADR 0269 later adds an optional display anchor
  beside each frame, and keeps that promise.
- The seam is not verifiable outside a packaged build (ADR 0016). The capture
  rule and the note are pure and covered by unit tests.

## Alternatives considered

**Suppress the flush arm for our own placement.** The obvious one, and it does
not work. The flush is armed by any window moving, by a navigation and by a
close, and the capture then reads EVERY window's live geometry. Suppressing one
arm only delays the write to the next one.

**Tell our own `Moved` event from the user's.** tao reports neither a cause nor
a source, so this needs a suppression window around each placement, which is a
threshold to keep calibrated. ADR 0204 refused a tuned threshold for the same
class of problem, and the frame comparison is exact where a window is not.

**Do not clamp at restore time at all.** It would keep every arrangement, and it
re-opens the shipped bug ADR 0193 exists for: a window restored 1x1 in a corner,
or off a display that is genuinely gone, with no way to reach it.

**Wait for the desk to settle before the restore clamp.** A quiet period like
ADR 0204's, applied at launch. It delays every launch for a case that cannot be
detected, and it still guesses wrong when a display takes longer than the wait.
The record holding firm covers the slow case and the genuinely-gone case with
one rule.

**Keep a second frame in the record, the "preferred" one beside the live one.**
It makes the record carry two answers, and every reader then has to choose. The
held frame IS the preferred one, so one field already says it.
