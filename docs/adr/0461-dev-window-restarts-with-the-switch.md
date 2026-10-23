# 0461: The Tauri dev window moves to a new build only at the switch, together with the gateway and the engine

- **Status**: Accepted
- **Date**: 2026-10-12
- **Amends**: the "watch is the default" choice in
  `docs/plans/2026-09-25-tauri-dev-window-rebuilds-on-app-changes.md`

## Context

`scripts/tauri-dev.sh` ran the dev window under `cargo tauri dev` with its
file watcher on, scoped by `crates/.taurignore`. Apply merges straight into the
main checkout, so the watcher rebuilt and relaunched the window the moment a
change to the app's Rust code landed. The engine moves to that change only when
the user presses "Switch to new version". Until then a newer window talked to
an older stack.

The merge also writes its files over a few seconds. On the day this was
reported, the watcher fired several rebuilds in a row and the window was gone
for about a minute.

Reconnaissance found two more gaps:

- **The dev switch never moved the gateway.** Switch restarts one workspace's
  engine through the gateway (`restart_workspace`). The background build
  publishes a new gateway binary too, but only the picker's reload button
  re-execed the gateway onto it.
- **Nothing built the window ahead of the switch.** The background build
  compiled the engine, the gateway and the CLI. Only the watcher built the
  window, at merge time.

The packaged app has neither gap. The window, the gateway and the engine come
from one bundle, and an update moves all three together.

## Decision

**One build, one switch, every binary.** The background build that Apply
starts also builds the dev window, and publishes it into `.launch/` beside the
engine and the gateway (ADR 0063). The switch adopts all three, in this order:

1. **The gateway**, when a newer one is on disk. It re-execs first, and the new
   image performs the workspace restart.
2. **The engine**, respawned by that gateway from the published binary.
3. **The window**, which follows the engine it shows. Once that engine started
   after the published window binary, the window re-execs onto it.

`tauri-dev.sh` no longer runs `cargo tauri dev`. It builds and publishes the
window binary, then runs that binary in the foreground. No file watcher
restarts the window, and `crates/.taurignore` is gone.

## Rationale

- **The window can never be newer than the stack.** The window decides from
  state, not from an event: a published binary that is not its own image, and
  an engine that started after it. A page reload, a missed event or a switch
  from another device all reach the same answer.
- **A switch that brings no window change leaves the window alone.** The build
  publishes the window binary only when its bytes changed, so most switches do
  not blink it.
- **Gateway first gives a clean order.** The window relaunches only on seeing
  the new engine, and only the new gateway can show it the new engine. No
  window loads against a gateway that is about to re-exec.
- **It reuses what exists.** The window already re-execs itself in place
  (`restart_process` in `lib.rs`). The gateway already re-execs in place
  (`reload_gateway`). The build already publishes atomically into `.launch/`.
- **ADR 0021 still holds.** `tauri-dev.sh` keeps refusing a worktree, and a
  worktree never has a published window binary, so its builds never compile
  the app.

## Consequences

- A change to the app's Rust code reaches the window at the switch, never at
  the merge. The Apply label already calls such a change a restart.
- The background build gets slower when the app's own inputs changed. It
  compiles the app only on a checkout that has run a dev window.
- A window build failure fails the build, as an engine failure does. A
  half-new version is the mismatch this decision removes.
- A switch with a newer gateway reloads the one gateway every workspace shares.
  Its engines keep running, and their clients reconnect.
- The window follows the workspace it shows. A switch on another workspace
  leaves it on its build until its own workspace switches.
- The dev window gets its URL at run time from the launcher, because one
  published binary now serves every workspace on the checkout.

## Alternatives considered

- **Keep the watcher and debounce it.** It ends the burst of rebuilds, but the
  window still moves at the merge, ahead of the stack. That is the main defect.
- **Relaunch `cargo tauri dev --no-watch` at the switch.** It compiles at the
  switch, so the window is gone for the whole build. It also needs a marker to
  tell a switch from a user closing the window.
- **Restart the window on every switch.** Simpler, but it blinks the window and
  resets its position on switches that brought it nothing.
- **Have the frontend restart the window when this device presses Switch.** A
  switch from the phone would leave the window behind.
- **Window only, gateway untouched.** It fixes the reported symptom. The
  gateway then stays on its old build until someone presses the picker's reload
  button, so the stack is still mixed.
