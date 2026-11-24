---
name: Notification Routing, §4.5: Service worker wedge (mitigation stack) (continued)
description: Part of Notification Routing. Its index lists the sections it covers.
---

One part of `system-knowhow/notifications`, which lists every part.

## §4.5: Service worker wedge (mitigation stack) (continued)

### Layer 3: Engine-scheduled wake-push (partial-wedge coverage, proactive)

Google's [web.dev push-notifications-common-issues guide](https://web.dev/push-notifications-common-issues-and-reporting-bugs) gives the canonical workaround for a wedged SW: send it a push. Every push event resurrects the worker, and queued `notificationclick` events drain as a side effect. Lucidos does this from the engine on a fixed delay, whether or not the user returns to the tab.

In `crates/lucidos-engine/src/scheduler/push.rs::send_push_to_all_with_app`, after fanning out the real push, the engine:
- reads each subscription's owning `devices.user_agent` and keeps macOS-Chromium devices (`is_mac_chromium`);
- dedupes per `device_id`, since a multi-tab device has several subscriptions sharing one SW;
- for each device, `tokio::spawn`s a task that sleeps `MAC_CHROMIUM_WAKE_DELAY` (3 seconds), then calls `scheduler::push::send_wake_push_to_device(device_id, notification_id)`.

The wake's payload is byte-identical to the original push plus `wake: true`.

The SW push handler (`sw.js`, `push` event) branches on `data.wake === true`. It still calls `showNotification`, because Chrome enforces `userVisibleOnly: true` and a skip counts against the silent-push budget. But it passes `renotify: false` + `silent: true`, and reuses the same `tag`.
- The *intent* is an in-place replacement with identical content (no re-pop, sound or banner) while dispatching the push wakes the SW.
- The replacement is **best-effort**. On macOS, Chrome hands notifications to the native Notification Center, which does [not reliably honour web-`tag` replacement](https://intercom.help/progressier/en/articles/6582394-how-can-a-web-push-notification-replace-previous-notifications-in-the-notification-tray). So a wake can show as a second banner.
- The read-skip below removes the common duplicate, where the user already tapped the original. Two banners briefly coexisting for a still-unread notification is the accepted cost.

**Skip when already read.** `send_wake_push_to_device` re-fetches the notification at fire time and returns `Ok(0)` *without sending* when it is `read` (`wake_still_needed` in `push.rs`).
- A tap during the delay means `notificationclick` drained and the SW was never wedged. The wake would only resurrect a handled notification as a fresh banner ("same push twice").
- Still unread means the user hasn't tapped yet, or a wedged SW swallowed the tap. Both want the wake.
- It must be a *fire-time* check, because the tap lands during the delay. Covered by `s4_5_wake_skipped_when_notification_already_read`.

**Why 3 seconds.** It is short enough that a tap feels immediate, and long enough that Chrome doesn't coalesce the two pushes into one dispatch. The wake must be a separate `push` event to resurrect the worker. A real trace shows the queued click draining inside this window while `visibility: 'hidden'`.

**UA gate is engine-side.** Device registration captures `devices.user_agent` (`POST /api/v1/devices/register`, called from `registerCurrentDevice` on every page load). See `crates/lucidos-agents-api/src/api/settings.rs::register_device` and `core::DeviceStore::register`.
- `PushSubscriptionStore::get_push_enabled` returns it alongside the subscription tuple, so the engine asks the page nothing per push.
- Non-Mac-Chromium devices (Safari, iOS, Firefox, Tauri, Chrome on Windows / Linux) skip the wake entirely.
- Devices with `user_agent = NULL` (legacy rows from before UA capture, or a registration race) are conservatively skipped; see `pick_mac_chromium_wake_targets`.

**Silent-push budget.** Chrome counts a push as "silent" only when `showNotification` is NOT called. The wake always calls it with the original title and body, so Chrome counts it as visible.

**Failure modes.** The spawned task lives for `~3 s + a web-push round-trip`.
- The captured `Arc<LucidosEngine>` keeps the engine struct alive. But dropping the tokio runtime on shutdown aborts the task at its next `await`.
- So surviving a restart depends on `axum_server::Handle::graceful_shutdown` with a 10 s drain outlasting the 3 s sleep. A wake fired in a restart's final ~3 s is aborted, and the next real push drains the queue.
- `Ok(0)` from `send_wake_push_to_device` means the notification was read meanwhile (the healthy read-skip), or the device unsubscribed. It is logged, not an error.
- Per-device failures are isolated (one tokio task each).

### Net behaviour

- **Safari 18.5+ on macOS / iOS.** Layer 1 is the intended tap path: the OS processes the declarative envelope.
  - The SW `push` handler still FIRES on iOS (the `[Client/sw] push` breadcrumb proves it), and its `showNotification` renders the visible banner.
  - **Do NOT retry skipping `showNotification` on iOS.** It was tested and produced NO notification at all. iOS uses the declarative fallback only when the SW push handler *errors or times out*, not on a clean resolve. The SW ALWAYS calls `showNotification`.
  - **Known limitation (running+icon-launched).** A tap on a PWA that is running AND was last launched from its home-screen icon can just focus it. This is an unfixed WebKit bug: `notificationclick` never fires and the declarative `navigate` is not applied. See the seventeenth iteration in History below.
  - On tap the OS navigates the window to `https://host/<slug>/?notification=…&thread=…&tap=…`, built from the stored subscription scope. The URL is cross-document, so the tap is a **full page reload**.
  - WebKit offers no reload-free channel, still true in Safari 26.6: neither `launchQueue`/`launch_handler: focus-existing` nor a same-document declarative navigate to an open window. The reload *is* the navigation, and the alternatives (hash URL, focus-only) drop the deep link.
  - The cross-document navigate and its page-side detection are a temporary measure (`docs/temporary-measures.md` § "Cross-document notification-tap reload on iOS"). Its removal condition is WebKit shipping a reload-free channel.
  - Three mitigations keep that reload cheap and keep it from READING as a relaunch:
  - **(1) Built deployments cache the graph.** The SW serves the immutable content-hashed `/assets/*` bundles Cache-first (`cacheFirst`) from `SHELL_CACHE` in `sw.js`. A reload pulls the JS/CSS graph from disk, and only the ~9 KB shell HTML round-trips.
    - The `/assets/*` branch refuses to cache an HTML response (`isHtmlResponse`), so a deleted bundle's SPA fallback never poisons a hashed entry.
    - The navigation shell (`index.html`) is **network-first** (`networkFirstShell`). It is keyed by a normalized `/` request, so every scoped `?notification=…` variant shares one entry.
    - Online, the shell is fetched fresh, so it always references the bundles the server has now. A cache-first shell once pinned deleted bundles and turned iOS PWAs black (the thirteenth iteration).
    - The fetch always passes `cache: 'no-cache'`. Chrome hands the worker a back/forward navigation as `force-cache`, and passed through, it booted the HTTP cache's shell of a build from hours earlier.
    - It falls back to the `install`-precached shell (fetched with `cache: 'reload'`) when offline or on a transient non-ok response (a 502 mid engine-restart). Only an ok, non-redirected response is ever cached, so a 502 serves the last good shell and never pins itself.
    - **Exception: any 503** is shown as-is. It is the gateway's "engine not serving" signal for a stopped, cold-booting or lazy-starting workspace (ADR 0014 §11).
    - A 503 carries the branded boot splash (ideally marked `X-Lucidos-Boot-Splash`) for a document navigation, or a plain "workspace stopped" body. A stale shell instead would 503-storm the down engine (a red connection dot, or a white screen).
    - The branch keys on the **503 status**, not the marker header. The gateway is a machine-global daemon that does NOT restart on a CC Apply, so the running one may predate the marker.
    - **Exception: the pairing screen.** The gateway answers an unpaired navigation with it in place, as an ordinary 200 marked `X-Lucidos-Pairing`. `networkFirstShell` and the `install` precache both refuse to pin it, through `isPairingShell`.
    - A redirect guard hands the browser a real `Response.redirect` rather than a stale shell. The gateway's `serve_pairing_shell` and `docs/plans/2026-08-19-nobody-is-stranded-by-the-pairing-update.md` explain why the screen is served in place.
    - The shell branch is gated on `IS_BUILT` + `mode === 'navigate'` + `path === '/'`. The Vite dev server stays network-fresh for HMR. App-UI iframe navigations under `/app/<id>/` are excluded, being their own server-rendered HTML.
    - The `/assets/*` branch self-gates by path. A Vite dev install serves unhashed modules under `/src`,`/@vite`,`/node_modules/.vite`, never `/assets/*`, and those must not be cached. So dev gets no shell or asset cache.
    - The windowed thread-list GET is deliberately NOT cached. `loadAllThreadsInner` only upserts and SSE has no resume cursor, so a stale list would strand threads deleted while away.
  - **(2) Dispatch on the next macrotask.** The page-side router dispatches the deep link right after boot (`setTimeout(handleHashLocation, 0)`), not after a fixed 500 ms delay.
  - **(3) The reload does not present as a launch.** A tap is a navigation inside a session the user never left. One captured `[Client/lifecycle] startup` line reads `dead_ms: 182`: the previous document was still heartbeating. So a document whose URL carries a `notification` key gets no launch ceremony.
    - An inline script in the `<head>` of `index.html` sets `data-boot-splash-quiet` on `<html>`, so the cover holds from the first frame. A user-requested refresh gets the same cover, via the one-shot `lucidos-splash-quiet` flag `refreshClient` stamps.
    - The decision cannot sit in the body. Every body script waits for the bundle stylesheet, and WebKit paints a frame the moment that lands. A body-side decision therefore shows one frame of the blue launch splash on every tap.
    - The quiet cover drops the brand mark and the gradient, clears the baked "Opening your workspace…", and shortens the leaving fade.
    - It paints the app's own `--bg-primary` on the splash AND both canvas layers. A fixed `inset:0` element never reaches the iOS standalone bottom safe-area strip.
    - `bootSplashPlaysNoReveal()` makes `useBootSplashReady` skip the `BOOT_SPLASH_MIN_REVEAL_MS` floor, as the gateway handover does.
    - The readiness gate (`connected && threadsLoaded`) is unchanged. The DELAYED status still writes past `STATUS_DELAY_MS`, so a stuck tap is quiet but not silent.
    - Measured on the reporting iPhone (iOS 18.7 / Safari 26.5, `reload_ms` 335-1132). It cut about 2.2-2.7 s of brand splash per tap to a flat hold of the real boot.
    - The gate mirrors `hasDeepLinkParams` exactly (a `notification` key, in query or hash), so the cover is quiet precisely when the router will dispatch.
    - A bare `thread=`/`event=` pair no-ops page-side and stays a launch.
    - The `#thread=<uuid>` cross-workspace landing channel keeps the launch splash, since that hop can lazy-start a stopped engine. The refresh flag is read before that branch, so an explicit refresh still quiets any hash.
    - It is NOT gated on iOS: a Chrome cold `clients.openWindow` tap is the same case.
    - The quiet cover is not part of the temporary measure above, since it also serves a user refresh.
    - **The gateway handover stands down.** For a tap on a STOPPED workspace, the gateway serves its own boot splash on that exact URL (query intact, meta-refreshed) until the engine answers.
    - The app document then arrives carrying the gateway's one-shot handover flag, and keeps the built mark standing (`boot-splash-formed`). Quieting it would snap away a mark the user has watched for seconds. So the quiet script reads that flag and bails before touching anything, canvas included.
  - **Mark-read AND navigation happen page-side.** When the URL lands, `handleHashLocation` → `dispatchDeepLink` reads the query params and calls `markReadOptimistic`. For navigate-kind taps it also calls `handleNavigationRequest`.
  - The router fires on cold start, warm `hashchange`, **and** every resume signal (`visibilitychange → visible`, `focus`, `pageshow`). A warm tap is a cross-document load caught by cold start. The resume signals catch iOS updating the URL while JS is suspended.
  - The dispatcher is idempotent: it strips consumed params after dispatch. See `crates/lucidos-app/src/store/actions/hash-deeplink-router.ts`.
- **Chrome on macOS today.** Chrome ignores `web_push: 8030`. The SW `push` handler parses the same envelope and dispatches via `showNotification` + `notificationclick`.
  - On tap, `routeToDeepLink` focuses the open **same-workspace shell** tab and `postMessage`s the deep link to it (see warm-tab routing above). With no shell tab open it `clients.openWindow()`s the scope-resolved hash URL.
  - Layer 2 catches the full wedge. Layer 3 sends an engine-side wake-push 3 s after every notification, so a wedged tap opens the right page about 3 s late.
- **Chrome the day [#382298314](https://issues.chromium.org/issues/382298314) ships.** Layer 1 takes over with no code change, since the declarative envelope is already on the wire. Layers 2-3 become defense-in-depth.

**Recovery is best-effort.** If `recoverServiceWorker` fails, the next probe retries after the 60 s cooldown. Layer 3 is the final user-visible safety net.

### Troubleshooting: push tap works on one device but not another (stale cached manifest)

**Symptom.** Tapping an OS push **navigates and marks read on one device** (a freshly installed PWA) but **silently does neither** on another. Both run the **same engine binary and frontend bundle**. The in-app router is fine (an inbox row or an "Open <app>" button works). The failure repeats per device, not at random.

**Root cause.** iOS Safari snapshots `crates/lucidos-app/public/manifest.json` at **"Add to Home Screen" time** and never refreshes it without a reinstall.
- iOS needs `launch_handler: { client_mode: "navigate-existing" }` to *navigate* a backgrounded PWA window on a push tap, not merely focus it. The field landed **2026-05-22**, with Layer 1.
- A home-screen install from **before** that date has a cached manifest **without** it. On a backgrounded tap, iOS focuses the window and leaves the URL untouched.
- So `handleHashLocation` → `dispatchDeepLink` never sees the `/?notification=…&tap=…` deep link. **Both** the navigation and its `markReadOptimistic` silently no-op, since they ride the same URL dispatch.

**Why it looks flaky.** The notification often flips to read later, when the Layer-3 wake-push or the next notification drains queued state.

**Why it's per-install, not per-version.** The frontend JS (`hash-deeplink-router.ts`, `parseDeepLinkFromUrl`, the resume listeners) loads fresh on every visit. Only the **manifest** and the **SW registration** are cached at install time, and the manifest gates open-window navigation.

**Diagnosis (no dev tooling).** Send the same notification shape (`Tap::Navigate { to.target = App }`) to a *fresh* install and the *suspect* one via `POST /api/v1/notifications`. If only the fresh one navigates and marks read, it's the cached manifest, not a code regression.
- The engine log line `[Push] Sent notification to https://web.push.apple.com/…` confirms the OS push fanned out.
- `Suppressed OS push` instead means an active device ponged and got an in-app toast. Background the PWA and retry.

**Fix.** Reinstall the PWA on the affected device. Remove it from the home screen, reopen the site in Safari, **Add to Home Screen** again, and re-grant notifications. A new APNs endpoint confirms the reinstall took and the manifest now has `launch_handler`. **Every device installed before the manifest change needs this**: no server-side mechanism forces iOS to refresh a cached manifest.

