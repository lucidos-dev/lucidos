---
name: Notification Routing, §1: Conceptual model
description: Part of Notification Routing. Its index lists the sections it covers.
---

One part of `system-knowhow/notifications`, which lists every part.

## §1: Conceptual model

Lucidos has **three notification surfaces** for the same `NotificationCreated` event:

1. **In-app surface**: the bell badge (unread count) and transient toast popups. The `NotificationCreated` SSE drives the badge on any connected page. The toast rides the `PresenceCheck` SSE (§3-§4): a page puts it on screen first, then reports that it did, and only that report suppresses the OS push.
2. **OS surface**: an OS-level banner outside the Lucidos UI. **Two transports, picked by client:**
   - **Web push** (browser / PWA): delivered by the device's push service (APNs / FCM) and rendered by the registered service worker.
   - **Native desktop** (Tauri app): a native macOS notification driven by the `NativePushRequested` SSE (§4). The app's `show_native_notification` command renders it and routes its tap, via Apple's `UserNotifications` framework (`UNUserNotificationCenter`). The embedded WKWebView can't subscribe to Web Push, so the engine reaches the app over its open SSE stream. Requires a packaged `.app` build (inert in `tauri dev`).

   Both transports ride the same server-side `push_allowed` decision (§3), on the push-ALLOWED branch. The decision is suppressed only by a page reporting it already showed the toast, auto-read the row, or held it with toasts off. So a page that hides after answering still has its toast, and the reader always gets at least one surface. Never web push *and* native.
3. **App-icon badge**: the unread *count* painted on the installed app's **icon**. That is a PWA's home-screen icon (Badging API), or the Tauri macOS **dock** tile plus that client's **menu-bar tray-icon title**. The tray title carries the count at all times, including with no dock tile. This is distinct from the in-app *bell* badge in item 1.
   - A PWA installed from a **gateway** origin shows the AGGREGATE total across running workspaces. That covers the picker at `/~/` and a workspace at `/<slug>/`, both re-stamped to `scope: "/"`.
   - The **Tauri desktop app** shows the same aggregate, whichever workspace is on screen.
   - A PWA installed from a **direct engine** origin (a legacy engine or an engine port at `/`) shows ITS OWN workspace's count.

   Unlike items 1–2, the `push_allowed`/PresenceCheck decision does not gate it: every device always shows its own count. See "App-icon badge" at the end of §4.

A device is in one of three **presence states** at any moment:

- **Active**: the user is looking at Lucidos right now. On desktop: `visibilityState='visible'` AND `document.hasFocus()`. On iOS PWA: `visibilityState='visible'` only, since Safari leaves `hasFocus()` false even when foregrounded.
  - **On the Tauri desktop client the *native window* must also be active (focused AND on-screen).** The WKWebView can't observe macOS `orderOut:`, so a window trayed to the menu bar keeps `visibilityState='visible'` and `hasFocus()=true`. Its `hasFocus()` is unreliable in general too.
  - Without the gate, a trayed or unfocused client reports Active and gets an invisible in-app toast instead of a native banner.
  - Rust bridges the authoritative AppKit state to the page via a `native-window-active` event (`crates/lucidos-app/src/lib.rs` → `utils/nativeWindow.ts`). `isPageActive()` returns false while it is inactive. The browser / PWA has no native bridge, so it is always Active-eligible.
- **Connected-but-hidden**: the SSE EventSource is still open but `isPageActive()` is false. Another tab is selected, or the window is behind another app. Or the iOS PWA is in the multitask switcher, or (Tauri) the native window is trayed or unfocused.
- **Offline**: no live SSE. The page is closed or unreachable, and only the OS surface can reach it.

A notification also carries a **source event** (`notifications.event_id`), so the page can check: "is the user looking at the very thing this notification points to?"

The rules below combine three concepts: **surface × presence × source-event-in-viewport**.

### Advising a user on enabling notifications: read the device first

A user may ask how to turn notifications on, why they "only get in-app toasts", or why a test push missed the OS. **First read the last used device in `[USER DEVICE & PREFERENCES]` (its `details:` line) and branch on the client. Do NOT assume a browser:**

- **`details: Lucidos desktop app …` → the native desktop app (Tauri).** Notifications arrive as **native macOS notifications** (the `NativePushRequested` SSE → `show_native_notification`, see §4).
  - There is **no browser, no in-app permission prompt, and no "site settings"**. macOS governs them under **System Settings → Notifications → Lucidos**. It asks once on first launch; if no banner appears, allow Lucidos there.
  - Flipping the `push_notifications` preference or settings switch is a no-op success on desktop (`initPushSubscription` short-circuits). The banner needs no web setup.
  - Native banners require a packaged `.app` build. A `tauri dev` build shows none.
  - As on every client, the OS banner fires only when this device is **not** the active one (§2). A focused window gets the in-app toast, so background the window to test the banner.
- **Any other `details` (a browser or installed PWA) → web push.** The **browser** asks for notification permission (click Allow; if missed, re-grant it from the 🔒 site-settings next to the URL). Delivery is via the device's push service and the registered service worker. The same active-device rule applies: an active page gets the in-app toast.

**Web push requires a secure origin (https or localhost).** A packaged headless install reached over the LAN is typically a plain `http://<host>:<port>/` origin. There the platform grants no service worker and no `Notification.requestPermission`, so push is impossible.

- **Enabling push says so** with an explicit toast. `pushUnsupportedReason` in `store/actions/push.ts` checks `window.isSecureContext` before the browser-support probes.
- **The toast names the gateway's own HTTPS address when it has one.** Behind a gateway with a *managed certificate*, it reads `https_origin` from `GET /~/api/v1/control/tls` and points there (`remote-access.md`).
- **Startup logs a console warning** (`store/startup.ts`), a telemetry carve-out with no per-load nag.
- **The fixes are origin-side, not settings-side.** Open Lucidos at the gateway's HTTPS address (`lucidos tls status`), at `https://localhost` on the gateway's own machine, or through `tailscale serve`. An SSH tunnel to `localhost` no longer works: the browser at the far end does not trust the host's *local certificate* (`remote-access.md`).

The `push_notifications` **preference** (the "on" switch in settings) only records that the user opted in. It is **NOT** the OS/browser permission, which Lucidos cannot toggle for the user in a browser. Don't conflate "the switch is on" with "the OS will show banners".

**Where that switch lives: two rows, one switch.**

- **Settings → Appearance & Behavior → Notifications** carries it for the device the user is holding. Point them there.
- **Settings → Devices** carries it per registered device. It is the only place to turn push off on a device they are *not* holding, since a page can only create its own subscription.
- Both rows go through one entry point, `setDevicePushEnabled` in `store/actions/push.ts`. So does the `PushNotificationRequested` prompt `preferences(action="set", key="push_notifications", value="enabled")` raises.
- The browser handshake runs first. A refused permission leaves the flag off rather than recording an opt-in the OS will never honour.
- Turning it on **from a phone** also offers to turn it off on the user's OTHER phones and tablets. One notification then stops buzzing several handsets. A desktop is never part of that offer, since a laptop and a phone are complementary surfaces. Cancelling leaves every device as it was.

### Remote workspace web push

Remote workspaces register the connecting gateway's installed worker at `/~/sw.js?workspace=<slug@peer>`.
Its scope is exactly that remote workspace; it handles push without intercepting requests or modifying caches.
Notification taps remain inside the selected remote workspace.
Startup waits for an active worker before refreshing the subscription.
Trusted entry retires legacy foreign workers and remote cached assets while preserving local workspace data.

