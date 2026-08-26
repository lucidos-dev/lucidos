# 0123: The window session restores the client's windows, keyed by workspace

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

A user with two workspaces open took an update. One window came back.

The engine half already works. A packaged restart tears the stack down, and the
teardown records the workspaces it stopped in `<app-data>/.next-boot.json`
(ADR 0014's topology; `crates/lucidos-gateway/src/next_boot.rs`). The next
gateway boot starts exactly those, whatever their `autostart` says.

Nothing did the same for the client's WINDOWS. Only `main` is declared in
`tauri.conf.json`, so only `main` is recreated on launch. Extra windows are
built at runtime by `open_app_window` with labels `window-<n>`, off a counter
that resets each process. `desktop::launch` navigates `main` to the gateway
root, and the page redirects to the workspace remembered in `localStorage`. So
exactly one workspace comes back, whichever the browser last recorded.

The same placement broke window geometry. `tauri-plugin-window-state` persists
by window LABEL. `window-1` names nothing across launches, and `main` is not
tied to any one workspace, so a second workspace's window could never keep its
size. `docs/plans/2026-08-14-open-workspace-in-a-new-window.md` listed that as a
non-goal: "Nothing remembers a window per workspace."

## Decision

The client keeps a **window session** record at
`<app-data>/.window-session.json`, keyed by workspace slug. It holds which
workspaces had a window, in restore order, and the last frame of each. A launch
reopens them: `main` takes the first, every other entry becomes a new window,
and each is sized before it is shown.

## Rationale

**The client is the only correct owner.** Only the client process can see every
window, read what workspace each is on, and create one. The gateway supervises
engines and has no idea a window exists. This is the same split ADR 0108 draws
for the update check, in the other direction: the check is per machine and
belongs to the gateway, and the windows are per client and belong here.

**Keyed by workspace, because that is the identity the user has.** A window
label is an implementation detail that changes between launches. A workspace
slug is what the user opened, what the URL carries, and what the gateway serves.
Keying on it is what makes "remember the size of each workspace's window" even
expressible.

**Separate from `.next-boot.json`, because they answer different questions.**
That record is a one-shot instruction from a teardown to the next boot. It is
consumed on read, and the *service* role writes it about *engines*. This one is
a standing arrangement, re-read on every launch, superseded only by the next
write. The *client* role writes it about *windows*. Merging them would put two
writers with two lifetimes on one file.

**Geometry outlives the window.** `open` is replaced on every capture, so a
window the user closed leaves it. `geometry` is merged, so reopening a workspace
weeks later still lands at the size it was left. Those are separate facts and
the record keeps them separately.

**The record says what the client is showing, and the gate only rules out
launches that show nothing.** `open` is replaced wholesale, so a capture taken
before there is an arrangement wipes it. Two launches are exactly that, and each
reached the record through a different writer.

- **Boot**, where every window is still on the bundled splash. The startup
  geometry write arms the debounced flush long before the first navigation.
  Ruled out by `any_window_is_navigated`.
- **A login start**, which comes up menu-bar-only and never shows a window,
  while `desktop::launch` navigates the hidden `main` anyway. Ruled out by
  `PresentedGate`, a latch set the first time a window reaches the screen.

Neither half asks whether a window is visible NOW, and a live visibility test
was tried and reverted. A hidden window is still part of the arrangement:
`main` is hidden rather than closed, and the tray brings it back on its
workspace. Testing visibility blocked the one write whose job is to SHRINK the
record, so closing your last visible window left the closed one in it.

Neither half asks for a workspace either. A window on the picker is a real
answer, so closing the last workspace window empties the set rather than
preserving a stale one.

**A teardown must not empty it.** Every deliberate exit destroys its windows one
at a time, and the `Destroyed` recapture would read that as the user closing
them. So each exit path records the arrangement and then sets `TEARING_DOWN`,
after which the recapture stands down. Without it the update relaunch would wipe
the record microseconds after writing it, which is the original bug restored by
its own fix.

**Restored geometry goes through the existing clamp.** `window_restore` was
written because a saved rect can be degenerate, or saved against a display that
is no longer attached. A second restore path would need the same guard, so it
uses that one rather than growing another.

## Consequences

- **A launch is no longer "open the last workspace".** It is "open what was
  open", so quitting with three windows and reopening gives three. That is the
  macOS convention, and it is what the report asked for. It is also a behaviour
  change on every launch, not only after an update.
- **The login launch is unaffected.** It comes up menu-bar-only with no window
  (ADR 0072), and `restore_plan` returns nothing for it.
- **A notification tap still outranks the session.** The user asked for that
  workspace a moment ago; the session is only what they had last time. The
  tapped workspace is then skipped in the restore, or it would open twice.
- **Two windows on one workspace collapse to one.** The record holds no
  per-window identity, so the second would come back on top of the first at the
  same frame.
- **A restored window on a stopped workspace shows the gateway's "workspace is
  stopped" page.** Accepted: it is the same page an already-open window shows
  when the user stops a workspace from the picker, so this adds no new state.
  Confirming liveness first would put a control call on the launch path.
- **Maximized and fullscreen are not per workspace.** They stay with the plugin,
  by label, for `main` only. Restoring a Space per window is a different problem
  and no report asks for it.
- **Dev writes nothing.** Dev shares the packaged app-data dir and restores
  nothing, so a dev run could only rearrange the packaged client's windows.
- **`window_restore::Rect` is now serializable**, because the record persists
  one per workspace. A second rect type would be a second set of units to get
  wrong.
- **Every extra window now carries the declared minimums.** `open_app_window`
  applies them, which File > New Window did not have before. The clamp reads a
  frame under the minimum as corruption. That inference is only sound for a
  window the user could not drag that small.
- **Close to Menu Bar did not preserve the arrangement. FIXED by ADR 0141.**
  Cmd+Q closed every secondary window, so the record shrank to `main`'s
  workspace and both the reopen and a relaunch gave one window. The park now
  hides every window instead, so nothing shrinks the record. The counter that
  held the record across a park stays retired: do not re-add it.
- **A notification tap during boot costs two remembered sizes.** `setup` sized
  `main` from the first recorded workspace, and a tap then points it elsewhere.
  The record stays accurate, since it describes the windows as they now are, but
  the two earlier sizes are gone and the user resizes once. Accepted: the fix
  needs the whole geometry map on the launch path, for a case that needs a
  banner tapped inside the boot window.
- **A frame can inflate once across a DPI change.** A window restored onto a
  monitor with a different scale factor is floored against the minimum in that
  monitor's points before it is moved. It is re-recorded at the inflated size.
  Accepted for the same reason: rare, self-correcting, and never inconsistent.

## Alternatives considered

**Let the window-state plugin do it.** It already persists geometry and it
already runs. Rejected on the key: it is per label, and the label is the thing
that means nothing across launches. There is no hook to relabel a window, and
`main` is declared in the config, so it cannot be renamed to a workspace at all.

**Label each window by its workspace (`ws-<slug>`).** That would give
per-workspace geometry from the plugin for free. Rejected on two counts. `main`
cannot take such a label, so the declared window still needs the special case.
And `desktop::gateway_capability` scopes the IPC grant to `main` and `window-*`
(ADR 0028). Widening that pattern to an arbitrary slug widens the ACL to
whatever a label can be made to say.

**Extend `.next-boot.json` with the window list.** One file, one restore.
Rejected on ownership. The service role writes it during teardown, and the
gateway consumes it once. The client writes this one throughout its life and
reads it on every launch. The service also has no way to know what the client
had on screen.

**Restore only across an update or restart relaunch.** Narrower, and it is
exactly what the report asked for. Rejected: the mechanism is identical, and a
launch that restores after an update but not after a quit is a rule nobody can
predict. Offered to the maintainer as the alternative scope and declined.

**Confirm each workspace is running before restoring its window.** It would turn
the stopped-workspace page into a skipped window. Rejected for now, on two
counts. It needs a control-plane call on the launch path. And the case arises
only when the user stopped a workspace they had a window on, which is already
that state today.

**Ask the frontend which workspace a window is on.** A command the page calls on
load. Rejected: the URL already says it, and
`window_target::window_workspace` already parses it for the notification-tap
targeting. A page-supplied answer is also a page choosing what gets recorded
about it.
