# 0054: The packaged client enables macOS private API so the app view's Fullscreen is real fullscreen

- **Status**: Accepted
- **Date**: 2026-08-07

## Context

The app view's Fullscreen control (`ContentHeaderActions.tsx::toggleFullscreen`)
asks the app panel for native fullscreen and falls back to a CSS
pseudo-fullscreen (`appPseudoFullscreen`, the `.app-ui-fullscreen` class) when
the request is unavailable or rejected. The fallback exists for installed iOS
PWAs, which genuinely cannot have the real thing.

The packaged macOS client always took that fallback, so an app "fullscreened" in
the `.dmg` install was a `position: fixed` panel inside a normal window, with the
window chrome, the Dock and the menu bar all still there. WKWebView ships with
WebKit's Fullscreen API **off**: `requestFullscreen` is only usable once the
`fullScreenEnabled` key is set on `WKPreferences`. wry sets it under its
`fullscreen` cargo feature (`src/wkwebview/mod.rs`), tauri forwards that feature
from its own `macos-private-api`, and we did not enable it.

`fullScreenEnabled` is a **private** KVC key on `WKPreferences`, with no public
documentation. wry says so at the call site.

## Decision

**Enable tauri's `macos-private-api` feature in `crates/lucidos-app`, and set the
matching `app.macOSPrivateApi` flag in `tauri.conf.json`.** WebKit element
fullscreen then works in the packaged window and the existing native path is
taken, so the desktop client's Fullscreen behaves exactly like a browser's.

Both switches are set on purpose. The Cargo feature is what compiles the
preference in; the config flag is what the Tauri CLI reads when it decides which
features to pass. Setting only one leaves `cargo tauri build` and a plain
`cargo build` / `cargo test -p lucidos-app` disagreeing about what is in the
binary.

No frontend change was needed. `toggleFullscreen` already prefers native,
`nativeFullscreenElement` already reads both the standard and `webkit`-prefixed
spellings, the header already listens for `webkitfullscreenchange`, and
`OverlayLayer` already portals the host's modals and toasts into the fullscreen
panel through `appFullscreenHost`. All of it was written for this and had simply
never been reachable on this client.

## Rationale

**The private-API cost lands where it does not hurt us.** Apple rejects private
API at *App Store review*. Notarization does not scan for it, and Lucidos ships a
notarized `.dmg` plus a headless tarball, with no App Store target and none
planned (see `CLAUDE.md` § One-Click Install: two shapes, neither of them a store
submission). So the concrete cost of this key is a dependency on Apple keeping an
undocumented preference working.

**And the failure mode of that dependency is benign.** If a future macOS drops
the key, `requestFullscreen` becomes unavailable again and `toggleFullscreen`
falls straight back to pseudo-fullscreen, which is exactly today's behaviour. The
control does not break, it regresses to what it already was, and the fallback
chain is untouched by this change precisely so that stays true.

**The alternative costs more than it saves.** The public-API route (below) buys
away a private key by adding IPC, a second source of truth for "am I fullscreen",
and a reconciliation path for window-fullscreen exits that Tauri does not report.
That is a permanent maintenance surface traded against a risk whose worst case is
a return to the status quo.

## Consequences

What we keep:

- The packaged client's Fullscreen is real fullscreen, and is the *same*
  mechanism as the browser's, so there is one behaviour to reason about and the
  host-overlay portal path is exercised on every client rather than only in a
  browser.
- No new Tauri command, no capability change, no frontend change. The ACL surface
  (`capabilities/*.json`, `desktop::GATEWAY_PERMISSIONS`) is untouched.

What we give up:

- **A private Apple API is now in the shipped bundle.** It is one KVC key, set by
  wry, and it forecloses an App Store submission without removing this feature
  first.
- **`wry/transparent` comes along with it.** `macos-private-api` enables both. We
  do not use transparent windows and this ADR is not permission to start: a
  transparent window is a separate decision with its own tradeoffs.
- **It cannot be verified by any test in this repo.** ADR 0016 records that macOS
  WKWebView exposes no WebDriver and `tauri-driver` is Linux/Windows only, so the
  packaged window cannot be driven. Verification is a manual click in a real
  `.dmg` build, and a regression here would be silent (the control keeps
  "working", it just gives the fake fullscreen again).

## Alternatives considered

**Native macOS *window* fullscreen instead.** Add a `set_window_fullscreen` app
command (the shape `toggle_window_maximize` already uses, a custom command
precisely so it sits under `allow-app-ipc` rather than the window-plugin ACL) and
have the Tauri branch of `toggleFullscreen` put the whole Lucidos window into a
macOS fullscreen space while setting `appPseudoFullscreen` so the app panel fills
it. Public API only, which is the entire appeal.

Rejected as the default because it is strictly more moving parts for a result the
existing code already knows how to produce: new IPC, a second source of truth for
fullscreen state, and a reconciliation path for the exits Tauri does not report
(the green button, Ctrl+Cmd+F, the swipe-away gesture), for which there is no
fullscreen-changed event, so it would have to be a poll or a `Resized` heuristic.
It also makes the packaged client's Fullscreen structurally different from the
browser's, forever. **This is the right escalation if the private key ever stops
working**, and it is written down here so the next person does not have to
rediscover it.

**Set the public `WKPreferences.isElementFullscreenEnabled` ourselves.** Reach the
`WKWebView` through tauri's `with_webview` and set the public property via
`objc2-web-kit`, avoiding the private key entirely. Rejected for now on cost, not
on principle: it means a new framework dependency and hand-written unsafe interop
run per window, to replace one line wry already ships, and it has the same
unverifiable-here problem. Worth revisiting if the private key becomes a real
liability rather than a theoretical one.

**Leave it as pseudo-fullscreen.** Free, and honest in the sense that it is what
the client does today. Rejected because the whole point of the control is to give
the app the screen, and on the one client that is a real desktop application it
was the only client that did not.
