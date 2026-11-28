# 0334: Two macOS defaults the desktop client overrides: a new window's position, and the service's exit timeout

- **Status**: Accepted
- **Date**: 2026-09-30

## Context

A user updated the DMG. A workspace window, left on the built-in display, came
back on the external one, showing the gateway picker. It was the fourth report
of the display half. ADR 0193, 0215 and 0269 each fixed a real defect in the
frame the client remembers. The frame was right every time.

The desk: an external display as primary at 0,0 5120x1440, and the built-in
below it at 1763,1440 1728x1117.

Two macOS defaults caused it. Both were measured outside Lucidos.

**AppKit moves a window born off the primary display.** tao builds the
NSWindow with `initWithContentRect:styleMask:backing:defer:` and no screen. A
standalone Swift probe with the same rect and style mask asked for 1763,1473.
It read back 643,-191 straight after init, and 643,30 once shown: the exact
numbers the client had logged on every launch.

tauri-runtime-wry does queue a move back to the requested point. But the
client clamped the window synchronously after `build()`. The clamp read AppKit's
invented frame, found its title bar on no screen, and rescued it onto the
primary. That correction was queued last, so it won. ADR 0215 then held the
record, and the same move repeated every launch.

**launchd kills a stopping service about 5 s in.** The service teardown
budgets 20 s for draining engines, on the belief that this was launchd's
default. A throwaway job that traps SIGTERM and drains for 15 s measured it.
Both `launchctl kickstart -k` and `launchctl bootout` SIGKILLed it about 5 s
after SIGTERM. With `ExitTimeOut` 20 in the plist, both waited out the drain.

The update's restart hit exactly that. The workspace's engine was draining a
coding agent session, and the teardown was killed inside its wait. So it never
wrote the restore record and never stopped Postgres. The workspace had no
autostart, so the new gateway left it stopped. The window's own request then lazy-started a
second engine, which waited 8 s on the draining one. That gap outlasted the
page's 10 s cold-start bounce, which sent it to the picker.

## Decision

**The client seats a newly built window at its frame itself**, synchronously
and before anything reads or shows it. The frame is judged before the build,
as `main`'s already is (ADR 0202), and no clamp follows the build.

**The service plist declares `ExitTimeOut`** from the same constant the
teardown budgets. The teardown also writes the restore record right after it
signals the engines, before it waits for them.

## Rationale

**ADR 0178's premise, "a window is born on the display its frame names", holds
only for the primary.** The builder still carries the frame, so the window is
the right size from birth. The seat corrects the one thing AppKit changes, the
position, in the same main-thread turn. So the window is never shown or read
anywhere else.

**A judgment made before the window exists cannot read a frame nobody chose.**
The post-build clamp could only ever see AppKit's placement. Judging first is
already how `main` works, so the two paths now agree.

**A budget launchd does not know about is a guess.** Declaring the timeout makes
the teardown's arithmetic and launchd's kill the same number, by construction.

**Writing the record first costs nothing and survives any later kill.** The ids
are known the moment the engines are signalled.

## Consequences

- A restored or cascaded window on a secondary display opens there, with no
  flash on the primary.
- A service restart waits for a draining engine, up to 20 s, instead of killing
  the teardown. An update with a busy coding-agent session takes that long.
- The plist changes, so the first launch of this build reloads the service
  definition once. That reload still runs under the old definition's timeout.
- A workspace without autostart that the teardown stopped comes back at the
  next boot, because the restore record now reaches the gateway.
- The page's cold-start bounce also stands down once its engine served it
  (`store/actions/connection.ts`). That covers an engine swap from any cause.
- Both platform facts need a packaged build to check by hand (ADR 0016). The
  flip, the plist key and the bounce decision are unit-tested.

## Alternatives considered

**Keep the post-build clamp and defer it until Tauri's move lands.** It would
still judge a frame after placing it, which ADR 0202 rejected. The show is
synchronous and the move is not, so the window would still flash on the
primary.

**Pass the screen to `initWithContentRect:…screen:`.** It works in a probe, but
tao owns that call. A tao patch would be ours to carry forever, for what one
synchronous seat does.

**Build without a position and place with `place_window` afterwards.** That is
the born-on-the-primary-then-moved shape ADR 0178 left, and its setters are
queued as well.

**Replace `kickstart -k` with a hand-rolled stop, wait, start.** launchd
already does exactly that once it knows the timeout. KeepAlive and the throttle
interval make a hand-rolled restart slower and racier.

**Have the client tell the gateway which workspaces it will restore.** The
restore record already carries that fact. It only needed to be written.
