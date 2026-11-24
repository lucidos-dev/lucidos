---
name: Notification Routing, §4.5: Service worker wedge (mitigation stack)
description: Part of Notification Routing. Its index lists the sections it covers.
---

One part of `system-knowhow/notifications`, which lists every part.

## §4.5: Service worker wedge (mitigation stack)

The OS surface (§1) depends on the service worker running `notificationclick` when the user taps a push. Chrome on macOS has a long-tail dispatcher bug that silently drops the SW handler ([Chromium #370536109](https://issues.chromium.org/issues/370536109), "Push Notifications `notificationclick` not handled in MacOS 15"). The OS notification stays on screen and the tap appears to do nothing. Two variants:

- **Full wedge.** The SW stops dispatching ALL events: no `push`, no `message`, no `notificationclick`. The liveness probe below catches it.
- **Partial wedge.** The SW still handles `push` and `message`, but drops `notificationclick`. The next genuine `push` resurrects the dispatcher, and queued clicks drain in a flurry ("clicked at T, navigated at T+113s after a new notification"). The liveness probe reads "healthy" here, because `message` still flows.

No single mitigation covers both, so Lucidos stacks three layers.

### Layer 1: Declarative Web Push envelope + `launch_handler`

Every push payload the engine emits conforms to the [W3C Push API "Declarative Web Push"](https://github.com/w3c/push-api/pull/385) wire format ([WebKit blog](https://webkit.org/blog/16535/meet-declarative-web-push/), shipped in Safari 18.5+):

```json
{
  "web_push": 8030,
  "notification": {
    "title": "...",
    "body": "...",
    "navigate": "https://host/<slug>/?notification=...&thread=...&event=...&tap=...",
    "tag": "<notification_id, user-<thread_id> for a push group, or lucidos-notification>",
    "actions": [{ "action": "unwatch", "title": "Unwatch" }],
    "data": {
      "notification_id": "...",
      "thread_id": "...",
      "event_id": "...",
      "app_id": "...",
      "tap": { "kind": "modal" | "navigate", ... },
      "navigate": "#notification=...  (HASH form, same params, for the Chrome SW)",
      "unwatch_thread_id": "..."
    }
  },
  "app_badge": 3
}
```

Built by `build_push_payload` in `crates/lucidos-engine/src/scheduler/push.rs`. `actions` and `unwatch_thread_id` appear only for a user-only message's push group, which `PushGroup::apply` stamps on inside the size fitting. The thread's driver gets the tag without the action, since a driver cannot unwatch. The SW passes `actions` to `showNotification`. A tap on Unwatch POSTs `/api/v1/threads/:thread_id/users/unwatch` and marks the notification read, and opens nothing.

The top-level `web_push: 8030` is the Push API spec's opt-in for declarative parsing (homage to RFC 8030). Safari 18.5+ recognizes the declarative fields. On iOS the SW `push` handler still fires and must still call `showNotification()` for a visible banner (see Net behaviour). On tap, the only reliable field before page JS runs is `notification.navigate`. `notificationclick` is not a reliable channel for installed iOS PWAs.

The top-level **`app_badge`** is the Declarative Web Push app-icon-badge field (sibling of `web_push` / `notification`). It is set at send time to the count that subscription's install should show: `0` clears the badge, and it is omitted only when no count can be read.
- It is the **only** way a *closed* iOS PWA updates its home-screen badge: iOS reads `app_badge` in its parent process WITHOUT running the service worker.
- On Chrome/Android the SW `push` handler reads it and calls `navigator.setAppBadge()`.
- The count is decided per subscription from its recorded `scope_url`. A gateway-served install gets the cross-workspace total, a direct-engine one this engine's own count, and a relayed one none (see "App-icon badge" above).

**The pushed body is size-capped; the notification row is not.** The web-push transport refuses a payload over **3052 bytes** of serialized JSON. `web-push` 0.11 checks `content.len() > 3052` on the plaintext handed to `set_payload`, *before* encryption. Its error text says "maximum payload size of 3070 characters exceeded", the wrong number and unit, so budgeting against 3070 still overflows. An unguarded long body dies inside `build()` and reaches **zero** devices, while the row and the bell badge still count it.

`fit_payload_body` in `crates/lucidos-engine/src/scheduler/push.rs` guarantees the fit before the transport sees it.
- It renders the envelope with an **empty** body and subtracts that length from the ceiling, less a small margin for JSON escaping. It truncates the body to what remains, re-measures, and shrinks again if escaping grew it.
- Nothing is hardcoded, because the overhead varies. The absolute iOS `navigate` URL comes from **this subscription's** `scope_url`, so two devices on one notification get different budgets. The Layer-3 wake adds a `wake: true` sibling, which makes it strictly tighter.
- The cut lands on a UTF-8 char boundary (real bodies carry emoji), snaps back to nearby whitespace when there is some, and appends `…`.
- Truncation logs `[Push] Truncated <kind> body to fit the 3052 B payload ceiling: …` with both lengths. The surviving `build()` failure arm says plainly that NO push reached that subscription.

**Only the body may shrink.** `web_push`, `notification.title`, both navigate URL forms, `tag`, `actions`, the whole `data` block and the top-level `app_badge` survive intact. Cutting any of them breaks the tap path. A body that fits passes through byte-identical. If the envelope alone exceeds the ceiling (an enormous title, never truncated), the body is dropped and the failure logged per subscription, with no panic. The **notification row always keeps the full text**: the push is a banner, not the content of record.

The PWA manifest declares `launch_handler: { client_mode: "navigate-existing" }`, so the OS reuses the existing PWA window (no new tab) when it navigates.

**Two navigate URL forms: absolute query for iOS, hash for the Chrome SW cold-open.** Same deep-link params, different carrier:
- `notification.navigate` (iOS Safari) is an **absolute query URL** built from the subscription's stored service-worker scope: `https://host/<slug>/?notification=…`.
- `notification.data.navigate` (the Chrome SW `notificationclick` handler) is a **scope-relative hash** URL: `#notification=…`.
- With no deep-link params, both are `.` (`navigate_url_ios` / `navigate_url_sw` in `scheduler/push.rs`). `resolveNavigate` in `sw.js` tolerates a leading slash or none.

The split is load-bearing. iOS Safari reuses an open PWA window on tap and does **not** apply a same-document (hash-only) navigation to it. It just focuses the window and the hash router finds nothing (*"tap nav to thread only focuses the app"*). A query string is a cross-document navigation iOS performs, so the page lands on the deep-link URL. It is absolute because real iOS devices applied no URL at all for a query-only relative `?notification=…`.

On the Chrome side the hash URL feeds **only** the cold `clients.openWindow()` path, where the new page's cold-start `handleHashLocation` reads it. A warm Chrome tab is NOT routed by URL: `routeToDeepLink` focuses it and `postMessage`s the deep link (see "warm-tab routing" below). The page-side `parseDeepLinkFromUrl` reads both hash and query, and hash wins when both carry a key.

**Warm-tab routing: postMessage, not `client.navigate()`.** When `notificationclick` finds an open top-level Lucidos tab, it `focus()`es it, which also unfreezes a Chrome-frozen page. It then `postMessage`s `{ type: 'lucidos:deep-link', target: <notification.data> }` to it. The page's `onServiceWorkerMessage` handler (`store/startup.ts`) runs `parseDeepLinkFromSwMessage` → `dispatchDeepLink`, the same router the URL path uses. iOS never reaches this code, since it handles the tap declaratively.

This is deliberately NOT a fragment-only `client.navigate('/#…')`. That call *succeeds* against the warm, SW-controlled tab, yet routes nothing:

- Chrome does not fire `hashchange` for a fragment-only `WindowClient.navigate()`.
- The page-side focus/visibilitychange resume safety net does not fire when the clicked tab was *already* focused and visible.

The symptom was a tab that focused and marked the notification read (the SW-side fetch) while the detail never opened. `postMessage` is independent of both `hashchange` and resume events, so it routes deterministically.

**Warm-tab routing targets the workspace shell, nothing deeper.** Behind the workspace gateway (ADR 0014) several workspaces share one origin under distinct prefixes (`/myws/`, `/dev/`, …). Each registers its own SW at `/<slug>/sw.js`, scoped to `/<slug>/`, and a push for `/myws` reaches the `/myws` SW. But `clients.matchAll({ type: 'window', includeUncontrolled: true })` returns **every** same-origin top-level tab, an open `/dev` tab included. So `routeToDeepLink` picks its candidate with `isWorkspaceShell(c.url)`, then focuses it and `postMessage`s the deep link.

**The shell test is an exact path match against `SCOPE_PATH`, never a prefix.** Another workspace's tab fails it, and so does a popped-out app tab (`/dev/app/<id>/`). The popout is same-scope and top-level, but serves the app's own HTML and listens for no deep link. With no shell tab open, the handler falls through to `clients.openWindow()` at the scope-resolved deep link, on the raising workspace. At the legacy root scope `/`, only a tab at `/` matches.

**A remote workspace's popout is a shell tab.** It opens the workspace shell at `/<slug>@<gateway>/#app=<id>` (ADR 0726), so it passes the shell test. A deep link can land in that tab, over the popped-out app.

**Stored subscription scope is load-bearing for iOS.** The page records the concrete service-worker scope URL (`scope_url`) when it subscribes or refreshes a push subscription.
- The API keeps it only when it matches the request's workspace prefix and, when the browser sends `Origin`, the same origin. It ignores malformed or cross-workspace values.
- The engine builds the absolute iOS `notification.navigate` URL from it, keeping `/<slug>/` behind the gateway.
- That avoids both known bad forms. `/?notification=…` escapes the workspace prefix to the origin-root picker. `?notification=…` is correct by URL resolution but iOS/WebKit did not apply it consistently.
- Legacy rows without `scope_url` use the query-only fallback until the next page load refreshes them.

**The browser subscription is reconciled against the engine's current VAPID key on every subscribe and refresh.** VAPID keys are per workspace: the `vapid_keys` table, minted on first use by `get_or_create_vapid_keys`, with the private key sealed at rest. Behind the gateway every workspace shares one origin and takes a `/<slug>/` scope. A workspace **recreated at the same slug** mints a fresh keypair while the browser keeps the old subscription at that scope. Unreconciled, the two push paths fail in opposite ways:
- `pushManager.subscribe()` rejects with `InvalidStateError` ("A subscription with a different applicationServerKey (or gcm_sender_id) already exists"). That surfaced as a dead-end *"Failed to enable push notifications"* toast.
- The silent page-load refresh re-POSTs the stale subscription. The engine then holds an endpoint whose push service rejects its VAPID signature, and notifications never arrive.

`ensurePushSubscription` (`crates/lucidos-app/src/store/actions/push.ts`) is the single chokepoint both paths go through.
- It byte-compares `subscription.options.applicationServerKey` against the engine's key. A match is returned untouched, a mismatch is unsubscribed and resubscribed.
- A browser that hides `applicationServerKey` reports "can't tell", not "mismatch". There `subscribe()` stays the authority, and one bounded retry after an `InvalidStateError` performs the same repair.
- The retry runs only when there is a subscription to drop, so an `InvalidStateError` raised for another reason cannot destroy a healthy subscription.
- Enabling push waits on `navigator.serviceWorker.ready` for the registration owning the **active** worker, as `recoverServiceWorker` does. `subscribe()` also rejects with `InvalidStateError` against a worker still `installing`.
- The follow-up `POST /api/v1/push/subscribe` clears the dead row, since `PushSubscriptionStore::subscribe` deletes by `device_id` before inserting.
- A device may only subscribe for itself. Behind the gateway the body's `device_id` must match the gateway-asserted `x-lucidos-device-id`, and may not be omitted. The engine answers a refusal as `200 {success: false}`, and the enable flow toasts it.
- It never unregisters the service worker as a last resort, which would throw away the shell cache.

Chrome / Firefox don't recognize `web_push: 8030` and pass the JSON straight to the SW `push` handler. The handler in `sw.js` reads `data.notification.title` / `body` / `tag` / `navigate` / `data` and calls `showNotification(...)`. It keeps the `navigate` option for when Chromium ships [#382298314 "Implement Declarative Web Push"](https://issues.chromium.org/issues/382298314). `notificationclick` then routes via the SW. Same payload, two handlers: Safari natively, Chrome via SW.

Safari honors `navigate` deterministically only inside the declarative envelope, not as a `showNotification` option. So this one wire format serves the iOS-PWA path AND the future Chrome path.

### Layer 2: Liveness probe + recovery (full-wedge coverage)

Ordinary API reads bypass the service worker; immutable blobs and the marked health probe still use it.
The page retries GET transport failures once. Resume requests a worker update only when the served build differs.
Worker pongs carry a temporary instance mark for diagnostics; recovery thresholds stay unchanged.

Every push calls `showNotification` with `requireInteraction: true`, so a dropped click leaves the notification on screen for a retry.

The page periodically pings the SW controller with `{ type: 'lucidos:ping' }`, and the SW replies `{ type: 'lucidos:pong' }`. The recovery is `recoverServiceWorker()`: `unregister`, re-register, re-subscribe push. It runs only after **two silent windows in a row** of `SW_PROBE_TIMEOUT_MS` (5 s) each, with the page awake through both. The rules live in `crates/lucidos-app/src/utils/swLivenessProbe.ts`. `SW_RECOVERY_COOLDOWN_MS` (60 s) gates the recovery, so flapping cannot churn the subscription endpoint. On success the page shows the toast *"Notifications repaired: the service worker was unresponsive"*.

**A miss is evidence only when the page stayed awake and the next ping also failed.** An iOS PWA once toasted the repair on resume with a healthy worker:

- **A probe the page was hidden for proves nothing.** iOS freezes a hidden page. On wake, the overdue timeout can run before the queued pong, so the probe reads a live worker as silent.
- **One silent window proves nothing either.** A worker cold-starting after a wake can miss the first ping and answer the next. A wedged worker misses every window, so a second probe still catches it.

**Probe trigger points** (`store/startup.ts`, `checkSwHealth`):
- 5 s after page mount (cold-start), after the mount-time fetch-path run below.
- On `visibilitychange → visible`, `window focus` and `pageshow`: the "user returns to Lucidos" path.
- Every `SW_PROBE_INTERVAL_MS` (5 min) while `document.visibilityState === 'visible'`. The interval starts and stops on visibility, so hidden tabs don't burn CPU wakes.

**Probe blindness on partial wedge.** The probe uses `message`, so it reads "healthy" when only `notificationclick` is broken. This layer cannot catch the partial wedge; Layer 3 exists for that.

**Fetch-path wedge.** A worker can answer the ping while its own `fetch()` never settles. Every `GET /api/v1/*` it forwards then hangs, and the page shows a skeleton forever. An iOS PWA hit this after a notification tap.
- `swFetchPathProbe.ts` runs at once on page mount, then after the ping probe at the other trigger points, one run at a time. A wedge strands the boot on its splash, so the mount run cannot wait. The probe requests `/api/v1/health` twice at once.
- `sw.js` skips the plain request but forwards the one marked `?sw-fetch-probe=1` (`FETCH_PROBE_PARAM`) through its `fetch()`.
- Plain answered and marked still silent after 4 s, in two windows in a row, page awake throughout: the page unregisters the worker and reloads. Both windows end inside the boot splash's 15 s cap. A failed plain request means the network is down, which proves nothing.
- A sessionStorage guard allows one such reload per 60 s, so an unfixable wedge never loops. The page logs a `sw` / `fetch_path_slow` breadcrumb when the first window misses, and `sw` / `fetch_path_wedged` before the reload.

**Admission stall.** Restore admission holds every gated request until the stream's hello passes a chain of device database steps (`store/restoreRuntime.ts`). A stall there also strands the boot splash, but the service worker is healthy.
- **Signature:** only the bootstrap URLs reach the engine (`/health`, `/devices/register`, `/internal/client-log`). No thread list or other read arrives.
- **Device database bounds** (`store/deviceDatabase.ts`): an open and each admission step may take 5 s (`DEVICE_DATABASE_STALL_MS`). A stalled step aborts its transaction and fails. Each stall doubles that database's bound, up to 80 s. A stall also closes its connection, so the next step opens a fresh one instead of reusing a wedged handle. The local-records step first waits for queued upload and unsent-message writes, up to the same 5 s. A stall there fails the step but leaves those writes running.
- **Retry:** a failed step reconnects after 3 s, and the new hello retries admission. A stall never opens admission. A first device storage stall retries silently. A repeat shows one "Device storage" warning with Retry, which admission removes. Any other failed step toasts "Workspace restore recovery could not finish".
- **Watchdog** (`store/restoreAdmissionWatchdog.ts`): a connection still waiting after 10 s counts as stalled. While it still waits for the hello, it restarts the stream once per connection.
- **Breadcrumbs:** `[Client/restore-admission]` logs `failed` (with the step), `stalled` (with the phase and wait) and `admitted_after_stall` (with the total wait). `[Client/device-database]` logs `open_blocked` when another connection blocks an upgrade.

