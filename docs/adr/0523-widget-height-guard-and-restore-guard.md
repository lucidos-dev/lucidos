# 0523: A widget height guard bounds a frame's height, and a restore guard keeps a launch off the thread the last session died on

- **Status**: Accepted
- **Date**: 2026-10-18
- **Related**: [0411](0411-every-workspace-has-home.md) (the no-stored-focus path
  a withheld launch takes).

## Context

An iPhone PWA restored one thread on every launch, and every launch froze. iOS
killed the page each time, and the phone stayed locked out until another device
moved the focus.

The thread embedded a widget whose `<html>` observer rewrote `class` on every
call. One SDK write to `<html>` started an endless observer loop. iOS WebKit
runs a cross-origin frame on the page's main thread, so the loop starved the
whole app. Chrome isolates the frame, so the Mac never showed it.

The SDK fix (a push writes `<html>` only when the host's value changes) removes
this trigger. It cannot stop the next widget that loops on its own.

## Decision

1. **Widget height guard.** `WidgetFrame` applies a height report only through
   a pure decision (`widgetHeightGuard.ts`). It ignores a change under 1 px and
   coalesces reports to one per animation frame. It locks a frame whose reports
   flip direction too often (`FLIP_LOCK_COUNT` in `FLIP_WINDOW_MS`), or grow by
   the same step for `CREEP_LOCK_FRAMES` frames in a row.
   The lock lasts for that mount and logs one `[Client/widget]` line.
2. **Restore guard.** Focusing a thread arms a device-local marker naming it.
   The marker clears after `RESTORE_GUARD_SURVIVAL_MS` visible, on `pagehide`,
   or when the page hides.
   A launch that finds the marker naming the stored focus lands on Home. It
   then offers the thread back once, in a toast with an "Open" action.

## Rationale

- **A lockout is worse than a lost restore.** A user who loses the restore
  taps "Open" once. A user locked out needs a second device.
- **The guard is device-local.** The crash is a property of one device's
  engine, so the marker never touches server state or other devices.
- **Re-arm on return to visible.** A resumed page remounts its widgets and can
  hang the same way, so it gets a fresh window too.
- **The height guard is cheap and pure.** A widget sized `100vh` plus a margin
  grows by 16 px per report forever. The guard stops it without knowing why.

## Consequences

- A thread that really crashed the page stays one tap away, never restored
  automatically.
- A page killed inside the survival window for any other reason also withholds the
  restore once. That costs one tap.
- A widget that hangs its own main thread still freezes the page on iOS. The
  restore guard keeps that from becoming a lockout. No host code can preempt a
  same-process frame.
- A locked frame keeps its height until it remounts, even if the widget later
  settles.

## Alternatives considered

- **Detect the loop in the SDK** with a DOM-state compare before each write.
  Rejected: re-adding a present class still queues a mutation record, so the
  widget's own write loops regardless. The SDK tracks what it last wrote instead.
- **Server-side crash flag on the thread.** Rejected: it would withhold the
  thread on every device, though only one crashed.
- **Never restore the focus on a phone.** Rejected: it gives up the restore for
  everyone to protect against a rare crash.
- **Lazy-mount or window the transcript harder.** Rejected: the widget was on
  screen at launch, so any mount strategy still runs it.

Plan: `docs/plans/2026-10-18-widget-loop-and-restore-guard.md`.
