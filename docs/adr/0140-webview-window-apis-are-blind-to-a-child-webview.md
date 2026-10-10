# 0140: Never use tauri's WebviewWindow APIs: a URL preview's child webview makes them blind

- **Status**: Accepted
- **Date**: 2026-08-26

## Context

The packaged desktop client is a MULTI-WEBVIEW app. A URL preview is a native
child webview, parked on the `main` window by `create_panel_webview` through
`Window::add_child`. It has to be: WKWebView cannot render an arbitrary remote
page inside our own document.

Tauri offers three flavours of the same handle, and only one of them tolerates
that. `Window::is_webview_window()` answers true only while every webview
attached to a window carries the window's own label:

```rust
pub(crate) fn is_webview_window(&self) -> bool {
    self.webviews().iter().all(|w| w.label() == self.label())
}
```

`main` fails that test for as long as a preview is open. Three surfaces are
gated on it, and each fails in its own quiet way:

| API | What it does with `main` |
|---|---|
| `WebviewWindow` as a command argument | refuses the call before the body runs |
| `Manager::get_webview_window(label)` | answers `None` |
| `Manager::webview_windows()` | omits it from the map |

Only the first says anything. The page gets the string `current webview is not
a WebviewWindow`, which names no command and suggests no action. The other two
report an app that has fewer windows than it has.

The client hit all three. `show_workspace_window` declared a `WebviewWindow`
argument, so clicking a workspace row raised that error whenever a preview was
open. Its `app_window_urls` enumerated `webview_windows()`, so the chooser
could not see the window it was meant to focus. It would have opened a second
window on a workspace that already had one, against invariant 1 of
`docs/plans/2026-08-25-opening-a-workspace-defaults-to-its-own-window.md`. Its
navigate arm resolved the target with `get_webview_window`, so repointing that
window failed with `no such window`.

A preview does not have to be VISIBLE for any of this. Every overlay hides the
child webview while it is open (`useHidePanelWebviewWhile`), so the Lucidos
menu is drawn over a preview that is merely hidden. A navigation used to leave
the child behind for good, so one preview earlier in the session was enough.

## Decision

**Nothing in `crates/lucidos-app` asks for a `tauri::WebviewWindow`**, neither
as a command argument nor from the manager. Use the flavour that matches the
operation:

- a window operation (show, focus, size, title, background) takes
  `tauri::Window`, found with `Manager::get_window` / `Manager::windows`;
- a page operation (read the URL, navigate, eval) takes `tauri::Webview`, found
  with `Manager::get_webview` / `Manager::webviews`;
- a command needing both declares `webview: tauri::Webview` and calls
  `webview.window()`.

Every app window is exactly one webview under a label of its own, so the two
enumerations agree on which app windows exist. `is_app_window(label)` is what
separates them from `url-preview-*` children, in both.

The ban is on ASKING THE MANAGER, not on the type. A `WebviewWindow` handed
back by `WebviewWindowBuilder::build()` is a real one by construction, and
`open_app_window` keeps using it. It reaches the window under it with
`as_ref().window()` for the one call that wants a window.

Two source scans in `lib.rs` enforce this, and both exist because the failure
is invisible to a reviewer:

- `no_command_takes_a_webview_window` reads every command declaration in the
  crate. That argument never appears at a call site, so the declaration reads
  correctly right up to the moment it refuses at runtime.
- `no_manager_lookup_asks_for_a_webview_window` reads every line of every `.rs`
  for the other two flavours. A blind lookup reads correctly too, and answers
  `None` in silence.

## Rationale

The `WebviewWindow` flavour is tauri's ergonomic default and is correct for the
single-webview app it assumes. We are not one, and no amount of care at a call
site changes that. The predicate is a property of the whole window, read at the
moment of the call. So the same line works all day, and then fails once a user
opens a link.

Choosing by operation rather than by convenience also puts the type at the
right altitude. `front_window` only ever shows and focuses, so `Window` is
honestly what it needs, and taking it makes the lookup total.

## Consequences

- The two paths a user reported are correct with a preview open. A workspace
  row opens the workspace, focuses the window already on it, and navigates it
  for a landing. A banner tap, a Dock click and the tray's Open item front
  `main` instead of building a second window beside it.
- The reopen path (ADR 0141) came in by merge and is covered with them, because
  it plans from `live_app_windows`. A planner that can see a preview-hosting
  window and a shower that cannot would leave that window parked.
- **The sweep is finished**
  (`docs/plans/2026-08-26-a-preview-lives-on-the-window-that-asked-for-it.md`).
  Twelve further lookups took the flavour their operation needs. Five were
  silent user-visible failures:

  | Site | What a preview on `main` cost |
  |---|---|
  | `close_all_to_tray` | skipped `main`, so the Cmd-Q park left it on screen (ADR 0141) |
  | `visible_app_windows` | counted it as gone, so that same park went `Accessory` with a window still up: no Dock icon, no Cmd-Tab entry, an unclickable menu |
  | `persist_window_session` | recorded a session without `main`, so the next launch forgot that window's workspace and frame (ADR 0123) |
  | the WKWebView crash watchdog | found no window and skipped the reload, so recovery was off exactly when a previewed page kills the content process |
  | `new_window_url` | read no URL off it, so File to New Window landed on the picker rather than the workspace you were on, undoing the fix released in 0.30.4 |

  `get_main_window`'s second lookup was unreachable and is deleted.
  `place_window` now takes the `tauri::Window` both callers already hold.
- **The preview slot is per window, so the whole family is owner-keyed.** A
  child is added to `webview.window()`, the caller's own, rather than parked on
  `main`. Every panel command takes the calling webview and resolves that
  window's own preview, `close_panel_webview` included. It was left ungated
  while a page could have somebody else's preview drawn over it, and per-window
  hosting is what removed the case.
- **Any app window can host a child now, which makes the sweep bind harder.** A
  blind lookup no longer merely misses `main`. It misses whichever window the
  user last opened a link in, so the two changes had to land together.
- The guard test is therefore widened.
  `no_manager_lookup_asks_for_a_webview_window` covers the two manager flavours
  beside the argument one. It could not exist before the sweep: twelve sites
  would have red-lighted the crate, and a gate that has to be switched off
  teaches nothing.
- **The gate reaches our crate, and nothing below it.** It is a source scan over
  `crates/lucidos-app/src`, so a lookup inside a DEPENDENCY is invisible to it.
  One is live: `tauri-plugin-window-state` enumerates `webview_windows()` in
  `save_window_state`, so a preview-hosting window keeps a stale `fullscreen`
  and `maximized` in the plugin's record. `docs/known-gaps.md` carries it.
- A preview is remembered under the window whose PAGE asked for it. That window
  is now also its host. So a navigation ends the preview it invalidated and no
  other, and there is no second window left to tell apart.

## Alternatives considered

**Keep `WebviewWindow` and close the preview before the call.** Rejected: it
makes every future caller responsible for a global precondition, and a hidden
preview is exactly the state nobody remembers to check. It also destroys work
the user can see, to satisfy an argument type.

**Stop parking previews on `main` and host each on its caller.** TAKEN, as the
follow-up change, once the sweep made it safe. It fixes a real misplacement: a
preview opened in a second window used to render over `main`. It is not an
alternative to the decision above, because the hosting window is blind either
way. It is what makes the blindness reachable from ANY window, which is why it
had to come second. It also needed the `panel-*` events to stop being emitted
to `main` by name, and they now go to the owner.

**Render previews in an iframe instead of a child webview.** That is what the
browser build does, and it is why the desktop build does not: most sites refuse
to be framed. Giving up the child webview gives up the feature.
