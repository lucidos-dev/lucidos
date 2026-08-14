# 0072: A packaged client relaunch goes through LaunchServices, so the new instance is activated outright instead of racing its dying parent for the front slot

- **Status**: Accepted
- **Date**: 2026-08-13

## Context

Two paths relaunch the packaged macOS client: the updater, after it has swapped
the `.app` bundle, and the Restart App action.

Tauri's `AppHandle::restart()` fork/execs the binary and exits. The new process
is never *activated* by the system. It lands in front only by inheriting the
front slot from its dying parent. It loses that inheritance whenever it
registers late with the window server. It then comes up behind every other
window, which reads as the app vanishing mid-update.

## Decision

`desktop::schedule_relaunch_after_exit` spawns a detached `/bin/sh` watcher that
waits for this process to disappear and then hands the bundle to LaunchServices
with `/usr/bin/open`. The caller exits once the watcher is armed.
`app.restart()` stays as the fallback for anything that is not inside a `.app`.

## Rationale

`open` launches the app the way a double-click does, so LaunchServices grants
activation outright rather than leaving it to a race.

The watcher has to wait for our exit rather than launching straight away.
`open` against a running app activates that instance, which here is the dying
one, and `open -n` would leave two clients overlapping. Waiting is what keeps
the relaunch to exactly one instance.

Three properties of the watcher script are load-bearing:

- **The wait is bounded.** A detached shell that could loop forever is the
  failure the bound exists to prevent. The ceiling is far longer than a
  shutdown takes, because giving up is giving up for good.
- **The launch is guarded by a second liveness probe**, not by falling out of
  the loop, which can also end at its ceiling. Launching there would aim `open`
  at a live process, which merely activates it. Nothing would then be left to
  bring the client back when it finally did exit.
- **Every path is quoted as one shell word.** The bundle path comes from
  `current_exe()` and can be anywhere the user dragged the app. A path or
  argument that is not valid UTF-8 is an error rather than a corrupted word,
  and the caller's fallback passes `OsString`s through faithfully.

## Consequences

- The relaunch is macOS-only. Elsewhere the function returns `Err` and the
  caller keeps its own respawn. That costs nothing, because macOS is the only
  packaged GUI shape Lucidos ships.
- Development needs no special case: an unbundled `tauri dev` binary has no
  enclosing `.app`, the bundle resolution fails, and the caller falls back.
- The relaunch argv drops `--login`. That flag is one-shot launch context, not
  a mode the process keeps. A client that came up at login and was later
  restarted would otherwise come back menu-bar-only. With no window to bring
  forward, it undoes the guarantee above.
- The exit that follows must be marshalled to the main thread. The updater
  command runs on the async runtime, so it routes through
  `exit_after_relaunch_scheduled`.

## Alternatives considered

- **`app.restart()` alone.** What we shipped, and what produced the
  behind-everything client. It is kept only as the fallback for an unbundled
  binary, where there is no bundle to hand LaunchServices.
- **Activate the new instance from inside itself** (`NSApp
  activateIgnoringOtherApps:`). Fights the window server rather than asking it,
  and the documented guidance is against apps stealing focus this way. It also
  cannot help a new process that is slow to register at all.
- **`open -n` without waiting.** Launches a second client immediately, so two
  exist for a moment. The dying one is the one holding the window the user is
  looking at.
- **`KeepAlive` on a launchd job for the client.** The login agent already
  exists and is deliberately one-shot: quitting the client must not respawn it.
  Reusing it for the relaunch would tie a user's Quit to a restart.
