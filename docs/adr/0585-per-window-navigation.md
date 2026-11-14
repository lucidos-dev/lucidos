# 0585: Navigation is per window: each window keeps its own focused thread, panel and history, across relaunch

- **Status**: Accepted
- **Date**: 2026-10-26

## Context

The desktop app can open several windows on one workspace. Every navigation
key (the focused thread, the open panel or app, the menu item, the history
stack) lived in `localStorage` under one device-wide name. So the windows
fought: a reload in one window restored whatever another had last shown. A
relaunch was worse. `window_session.rs` keyed its record by workspace slug, and
extra windows got labels `window-<n>` off a counter that reset each launch. No
window could find its own state again.

Engine navigation had the same flaw. `NavigationRequested`,
`AppUiCaptureRequested` and the OAuth completion toast are device-scoped. Every
window of the workspace on the device received them, and every window acted.

## Decision

Navigation belongs to a window. Each window has a **window id** and keeps its
navigation in **window-scoped storage**. The windows of a device coordinate as
**window peers**, so exactly one acts on a device-scoped event. Preferences and
layout stay shared.

- **Window id.** On the desktop it is the Tauri label, read synchronously at
  boot. In a browser it is a per-tab id in `sessionStorage`, which a reload and
  a browser session restore keep (`utils/windowStorage.ts`). A duplicated tab
  copies that id, so the younger of the two takes a new one.
- **Window-scoped storage.** A key is stored as `<key>@window:<id>`. Every
  write also lands on the plain key, the **shared seed**. A window with no state
  of its own, such as a brand-new one, starts from the seed. A removal writes an
  empty own value, a tombstone, so a window that went back to Home does not
  fall through to another window's seed. A window unseen for 30 days is pruned.
- **Window peers.** The windows announce what they show over a
  `BroadcastChannel` named per workspace (`store/windowPeers.ts`). Each one runs
  the same pure `chooseWindow` over the same peer list, so all agree on one
  winner without a round trip. Each page load holds a Web Lock for its life,
  and its peers wait on it, so a window that crashes drops out. A window still
  counts for what it showed for two seconds after a change. The winner often
  changes what it shows at once, and a slower peer must still see it win.
- **Restore guard.** The marker also names the window that armed it. A window
  that found another window's marker through the seed releases the thread once
  that window shows it is alive.
- **Desktop labels.** An extra window is labelled `window-<uuid>`, which never
  recurs. The session record holds one entry per window: label, workspace and
  frame. A relaunch rebuilds each window under its recorded label, so the page
  finds its own window-scoped storage again.

## Rationale

The label is the only id the page can read before its first render. The focus
signal is initialized at module load, so an id fetched asynchronously would
arrive after the wrong thread had already drawn. So the label must be stable,
and the counter labels had to go.

The shared seed keeps a new window useful. Without it, File > New Window would
open on Home every time, where the user expects to land where they were.

Choosing by a deterministic function, rather than electing a leader, means no
window has to stay alive for the others to act. A closed or crashed window just
drops out of the next choice.

## Consequences

- A reload, a crash reload and a relaunch each restore what THAT window showed.
- An old session record, listing bare slugs, still restores every workspace it
  names, under fresh labels. Those windows start from the seed once.
- `AppUiRefreshRequested` still reloads the app in every window: it is about
  the app, not about navigation.
- Preferences and pane layout stay device-wide. A user who resizes a pane in
  one window sees it in the next.
- Storage grows by one copy of each navigation key per window, bounded by the
  30-day prune.

## Alternatives considered

- **Keep navigation device-wide and only fix engine events.** This was the
  first, narrow plan. The user rejected it: a relaunch must give each window
  back its own thread.
- **Per-window storage with no seed.** Simpler, but a new window always opens
  on Home, and so does every window after an upgrade.
- **Store navigation server-side per window.** A window id is a client fact
  with no lifecycle on the server, and every navigation would cost a round trip.
  `localStorage` is already the device's navigation store.
- **Elect a leader window for device-scoped events.** It needs a heartbeat and
  a hand-over when the leader closes. The deterministic choice needs neither.
- **Keep counter labels and map them to slots at relaunch.** Slot `window-1`
  would inherit whatever window last held that number, including one the user
  closed. A label that never recurs cannot inherit a stranger's state.
