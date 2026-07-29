# 0028 — The packaged window is a *remote* origin to Tauri's ACL, and is granted it explicitly

- **Status** — Accepted
- **Date** — 2026-07-29

## Context

The packaged macOS client is not a normal Tauri app. `frontendDist: "./dist"`
bundles a small boot splash, so Tauri's *app URL* is `tauri://localhost`; but the
real frontend is served by the always-on gateway, and `desktop::launch` navigates
the main window to `http://localhost:<port>` as soon as the service is healthy
(ADR 0014). From then on the window is on an origin that is not the app URL.

Tauri 2.10.2 did not care. `Webview::on_message` only consulted the ACL for
plugin commands or when the app defined its own ACL manifest, and we defined
neither — our commands were simply unchecked. Tauri 2.11 added a third arm:

```rust
// 2.10.2                              // 2.11.x
if (plugin_command.is_some()           if (plugin_command.is_some()
    || has_app_acl_manifest)               || has_app_acl_manifest
                                           || !is_local)
```

with the rationale *"remote content can never reach custom commands unless an
explicit `remote` capability has been configured for them."* `is_local_url()` is
false for our window, `Origin` is derived from the `Origin` header of the `ipc://`
request (`tauri/src/ipc/protocol.rs`), and our only capability was local-only —
so **every** IPC call from the packaged app started being rejected with
`Command <x> not allowed by ACL`. The lockfile moved 2.10.2 → 2.11.4 in an
in-range bump (`de287a93a`, 2026-06-30) and shipped in v0.16.0.

The damage was total and silent: no native notification banners, no `listen()`
(so no SSE-driven surfaces at all), no window drag, no updater — and, because the
JS heartbeat is itself a command, a webview reload every 60s forever (6232 of
them in one install). Dev was unaffected the whole time, because `devUrl` **is**
the app URL there, so dev pages take the local branch. Only a packaged build ever
exercises the remote path.

## Decision

**The upstream change is right, and we adopt it rather than pin around it.** A
window pointed at an HTTP origin genuinely is remote; the fact that the origin is
ours is a claim only we can make, and the ACL is where we make it.

Concretely:

1. **The app declares an ACL manifest** (`crates/lucidos-app/permissions/`). This
   is not optional dressing — the resolver keys app commands off an app
   permission, so there is no way to allow one from a remote origin without a
   manifest. The cost is that `has_app_acl_manifest` becomes true, which means
   every app command is now ACL-checked on *every* origin, dev included. An
   omission is therefore fatal everywhere, which is why a test asserts the
   permission list and `tauri::generate_handler!` cannot drift.

2. **The gateway capability is registered at runtime, pinned to the resolved
   port.** The port is per-install (`<app-data>/config/engine-port`), so a static
   `capabilities/*.json` could only say `http://localhost:*` — and that hands our
   IPC surface to any other local HTTP server the window could be navigated to.
   `desktop::launch` already resolves the port immediately before navigating, so
   it builds the capability there via `Manager::add_capability`.

3. **Capabilities are scoped by `webviews`, never `windows`.** A `windows`
   entry enables a capability on *every* webview of the matching window, and the
   `url-preview-*` webviews that display arbitrary third-party sites live inside
   the main window. Window scoping would therefore invert the intent of any
   remote context attached to it. A unit test enforces that no capability
   combines a `remote` context with window scoping.

4. **The three panel-report commands get their own any-origin capability.**
   `__panel_title_report` / `__panel_url_report` / `__panel_content_report` are
   invoked by injected JS running *in the previewed page*, so the caller is
   untrusted third-party content by construction and no narrower origin pattern
   exists. They are granted alone, on `url-preview-*` webviews alone. This is a
   large net tightening: before 2.11 those same pages could invoke every command
   we had.

## Alternatives considered

- **Pin or bump tauri.** Rejected outright: it re-opens a hole upstream closed
  deliberately, and an in-range bump would silently re-break it later.
- **Make the origin local again by pointing `frontendDist` at the gateway URL.**
  Impossible in practice — `frontendDist` is static and the port is per-install —
  and it would also delete the bundled "Starting Lucidos…" splash the client
  shows while the service boots, since there would be no local asset to load.
- **Register `http` as a custom URI scheme protocol** (the third arm of
  `is_local_url`). That hijacks all HTTP in the webview.
- **A static capability with `http://localhost:*`.** Simpler and build-validated,
  but it grants the app's full IPC surface to any localhost origin. The runtime
  form costs one `add_capability` call and a test that resolves the same object.
- **Per-command app permissions** (`allow-heartbeat`, …) instead of one
  `allow-app-ipc`. Both capabilities grant the identical set today, so the
  granularity would buy nothing and double the surface that can drift.

## Consequences

- Adding a `#[tauri::command]` now requires adding it to
  `permissions/app-ipc.json`. Forgetting fails
  `acl_tests::app_permissions_match_the_invoke_handler`, not the packaged build.
- Secondary `window-*` windows gained the plugin permissions they never had (the
  old capability was `windows: ["main"]`), so `listen()` works in a New Window.
- The regression class — a working page that cannot reach Rust at all — is now
  visible without a debugger: `invoke` reports it as `[Client/ipc]` lines in
  engine.log, and the heartbeat watchdog backs off and says what a futile reload
  means instead of thrashing once a minute.
- What is still only provable on a packaged build: that WKWebView's `Origin`
  header for our window is exactly `http://localhost:<port>`. The ACL decision
  for a given origin string is unit-tested; the string the OS produces is
  established by reading `parse_invoke_request` and by the field evidence.
