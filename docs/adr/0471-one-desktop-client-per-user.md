# 0471: The packaged desktop client holds an owner lock; a second client exits without stealing focus

- **Status**: Accepted
- **Date**: 2026-10-14

## Context

A user found two packaged clients on screen, both showing every workspace. The
backend was single: one gateway, one engine per workspace. The extra process was
`Contents/MacOS/lucidos-app --version`, run by a coding agent to read the
installed version. The binary knew only `--service`, so it booted a full client.

LaunchServices keeps a Finder, Dock or `open` launch of one bundle to one
client. A direct exec of the inner binary bypasses it, and so does a second
copy of the bundle at another path. Two earlier plans
(`2026-08-03-relaunched-client-comes-back-frontmost`,
`2026-08-06-macos-client-returns-to-the-menu-bar-at-login`) declined a
single-instance guard, because the relaunch overlap they fixed was the only
known path to two clients. This was a second path.

## Decision

The packaged GUI client takes its *owner lock* (ADR 0438),
`<app-data>/client.lock`, in `main`, with one non-blocking try before any Tauri
or AppKit init. A process that loses the try prints the holder's published pid
to stderr and exits 0. Separately, `--version` / `-V` print one line and exit
before the lock.

## Rationale

- **The refusal moves no focus.** The usual caller is a script or an agent,
  and activating the running client would interrupt the user mid-task. A human
  who opens a second bundle copy sees nothing new, and the running client
  stays in the menu bar.
- **Before Tauri init.** A refused process never creates a window, a tray item
  or a Dock icon.
- **The owner lock cannot go stale.** The kernel drops it when the holder exits
  or crashes, so no pid liveness check is needed. It is the same mechanism as
  the engine and the gateway, through the same crate.
- **Close-on-exec keeps the exit-first relaunch paths working.** The
  LaunchServices watcher waits for the old pid to exit. The in-place fallback
  execs, and the closed fd lets the new image take the lock again.
- **A spawn-first relaunch releases the lock first.** Tauri's `restart` starts
  the new client before the old one exits, so the updater's fallback drops the
  lock before calling it.

## Consequences

- At most one packaged client per app-data dir. Two gateways on one machine
  are unaffected: the lock guards the client, not the backend.
- Dev builds and the `--service` role take no lock.
- A stray flag can no longer produce a second client, whatever it is.
- The refusal goes to stderr before the client log exists. A LaunchServices
  launch discards it, so a refused second bundle copy leaves no trace.

## Alternatives considered

- **`tauri-plugin-single-instance`.** It activates the first instance and
  forwards argv, which is the focus theft ruled out above. It also runs after
  Tauri init, so the refused process has already built AppKit state.
- **A new flock module in the app crate.** The owner-lock crate already owns
  this pattern, its pidfile rule and its probe caveats.
- **Fix `--version` alone.** It closes this trigger only. Any other direct exec
  of the binary would still open a second client.
- **Activate the running client, as LaunchServices does.** Right for a human,
  wrong for the scripts and agents that reach this path most.
