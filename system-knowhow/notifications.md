---
name: Notification Routing
description: How Lucidos decides whether a notification reaches the user as an OS push, an in-app toast, a bell-badge increment, or is silently auto-marked-read: the two surfaces, device presence states, and the PresenceCheck protocol. Load for "why didn't I get a push on my X" or cross-device noise.
---

# Notification Routing

This is the canonical spec for what happens when a `NotificationCreated` event is emitted. It says who gets an OS push, who gets an in-app toast, whose bell badge changes, and what gets auto-marked-read. The rules apply to every notification source: `send_notification` from the chat LLM, scheduler errors, trigger output and push-from-trigger.

Tests carry the section IDs below (`s2_scenario_4_tab_in_background_tab_push_fires`), so a failing test points at the broken rule.

## §1: Conceptual model

Lucidos has **three notification surfaces** for the same `NotificationCreated` event:

1. **In-app surface**: the bell badge (unread count) and transient toast popups. The `NotificationCreated` SSE drives the badge on any connected page. The `NotificationToastRequested` SSE drives the toast, and the engine emits it only after it decides to suppress the OS push (§3-§4).
2. **OS surface**: an OS-level banner outside the Lucidos UI. **Two transports, picked by client:**
   - **Web push** (browser / PWA): delivered by the device's push service (APNs / FCM) and rendered by the registered service worker.
   - **Native desktop** (Tauri app): a native macOS notification driven by the `NativePushRequested` SSE (§4). The app's `show_native_notification` command renders it and routes its tap, via Apple's `UserNotifications` framework (`UNUserNotificationCenter`). The embedded WKWebView can't subscribe to Web Push, so the engine reaches the app over its open SSE stream. Requires a packaged `.app` build (inert in `tauri dev`).

   Both transports ride the same server-side `push_allowed` decision (§3), on the push-ALLOWED branch. The in-app toast lives on the push-suppressed branch. So a notification reaches one device through exactly one surface: never a native banner *and* a toast, never web push *and* native.
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
- **Startup logs a console warning** (`store/startup.ts`), a telemetry carve-out with no per-load nag.
- **The fixes are origin-side, not settings-side.** Open Lucidos over https://, over `http://localhost`, or through `tailscale serve` (trusted HTTPS on the tailnet). An SSH tunnel to the host counts as localhost, which is a secure context.

The `push_notifications` **preference** (the "on" switch in settings) only records that the user opted in. It is **NOT** the OS/browser permission, which Lucidos cannot toggle for the user in a browser. Don't conflate "the switch is on" with "the OS will show banners".

**Where that switch lives: two rows, one switch.**

- **Settings → Appearance & Behavior → Notifications** carries it for the device the user is holding. Point them there.
- **Settings → Devices** carries it per registered device. It is the only place to turn push off on a device they are *not* holding, since a page can only create its own subscription.
- Both rows go through one entry point, `setDevicePushEnabled` in `store/actions/push.ts`. So does the `PushNotificationRequested` prompt the LLM `enable_push_notifications` tool raises.
- The browser handshake runs first. A refused permission leaves the flag off rather than recording an opt-in the OS will never honour.
- Turning it on **from a phone** also offers to turn it off on the user's OTHER phones and tablets. One notification then stops buzzing several handsets. A desktop is never part of that offer, since a laptop and a phone are complementary surfaces. Cancelling leaves every device as it was.

## §2: The fan-out rule

**Step A: global push check (once per notification).** Is any device active right now?

The engine runs the PresenceCheck protocol (§3) to find out. After Step A, it has a boolean `push_allowed` and zero or more `PresencePong` records.

- `push_allowed = true` if and only if NO device returned `is_active: true` within the deadline.
- The engine skips the protocol and sets `push_allowed = true` directly only when **nobody is reachable**. That means zero open SSE connections (`GET /api/v1/events`) AND zero `device_presence` candidate rows fresher than `PRESENCE_STALE_AFTER` (120s, `crates/lucidos-engine/src/core/device_presence.rs`).
- The live SSE-connection count is the primary gate, and the heartbeat candidates are a secondary signal (see §3 "Trigger").

**Step B: per device, in this exact order. First match wins:**

| # | Condition | OS push | In-app toast | Bell badge | Auto-read |
|---|---|---|---|---|---|
| 1 | Active AND on source thread AND source event in viewport | n/a¹ | no | no | yes |
| 2 | Active AND on source thread AND source event NOT in viewport | n/a¹ | yes | yes | no |
| 3 | Active AND on Lucidos, NOT on source thread | n/a¹ | yes | yes | no |
| 4 | Connected-but-hidden (SSE alive but `isPageActive()` false) | `push_allowed`² | no³ | yes | no |
| 4b | Connected, visible, but an app is FULLSCREEN | `push_allowed`² | no⁴ | yes | no |
| 5 | Offline (no SSE) | `push_allowed`² | n/a | re-syncs on next page load via `GET /api/v1/notifications?filter=unread` | no |

¹ Active devices never get an OS push by definition of Step A: at least one device (this one) is active, so `push_allowed = false` for everyone.
² Only sent when Step A returned `push_allowed = true`. Otherwise no push to this device either.
³ The page gets the `NotificationCreated` SSE and increments the bell badge, but renders NO toast. A toast would auto-dismiss before the user returns, a "ghost toast" they'd miss. The bell badge is the durable, on-return signal.

⁴ The page is visible, but the shell is not the surface in front of the user. It pongs `is_active: false`, so the notification takes the OS surface (see §4 for why). A fullscreen app thus agrees with a popped-out app window, which has always been Row 5. Rows 1 to 3 all require an ACTIVE shell.

**OS-push transport: web push vs native desktop (Rows 4 & 5).** "OS push" in the matrix is a *surface*, not one transport:
- **Browser / PWA** → web push (APNs / FCM) to the device's stored `push_subscriptions` row, rendered by the service worker.
- **Tauri desktop app** → a native macOS notification. When `push_allowed = true` the engine broadcasts a `NativePushRequested` SSE. The connected desktop page renders the banner and routes its tap via `show_native_notification` (§4). Both transports fire on the same branch, so the per-device matrix is unchanged.
- A **fully quit** Tauri app is Row 5 (offline), with a caveat. The bundled desktop build ships the engine *inside* the app, so quitting the app stops the engine too. Native delivery therefore covers Row 4, and Row 5 only while a separate engine is still reachable. A quit single-process install re-syncs the bell badge on next launch.

**The OS-surface decision runs for any connected client, not just web-push subscribers.** `send_push_to_all_with_app` runs the PresenceCheck decision (and so the toast or native-banner emit) whenever any client is reachable. Reachable means a web-push subscription **or** an open SSE connection or fresh heartbeat. So a desktop-only workspace with zero web-push subscriptions still gets in-app toasts and native banners. The engine does nothing only when there is no subscription **and** nobody connected.

**The fan-out never holds up whoever created the notification.** `send_push_to_all_with_app` starts the fan-out on its own task and returns at once. A trigger, a backup run or the event bus goes on while pushes are in flight. That task keeps the caller's event-trigger chain depth.

**A quiet device never delays another.** The web pushes go out to every device at once. Each send gets 30 seconds (`PUSH_SEND_TIMEOUT`), because the transport sets no deadline of its own. A send that runs out of time fails for that subscription only: the engine logs it and keeps the subscription (silence is not a 410).

**Row 1 requires a non-null `event_id`.** `notifications.event_id` is nullable. Scheduler errors, ad-hoc `send_notification` calls without an event ref, and audit events have no source event. With a null `event_id`, the device falls through to Row 2 (if the thread matches) or Row 3.

**Multi-tab / multi-window per device.** Two Lucidos tabs on the same browser share `device_id` (per-origin localStorage). Where the browser has `SharedWorker` they also share **one** SSE connection, held by the *shared SSE holder*. The holder ORs their answers into the single pong that connection owes. Otherwise each tab holds its own connection and POSTs its own pong. The reconciliation is the same either way:
- For Step A's `is_active`: logical-OR across the device's answers. If ANY tab on a device is active, the device counts as active.
- For Step B's auto-read (Row 1): if ANY tab reports "event in viewport on source thread", the auto-read endpoint is hit once. `POST /api/v1/notifications/:id/read` is idempotent.
- Step B for the in-app surface runs **per tab**. Each tab checks its own focused thread and viewport on SSE receipt.

**Where the OR happens is what keeps the count honest.** The engine waits for one pong per open SSE connection, so N documents behind one connection owe one answer, not N. Aggregating in the holder makes that true by construction. Per-document answers would let a background tab settle the push while a foreground one was still answering.

**Worked example.** macOS Chrome with Lucidos in a background tab; iOS PWA with the screen off in your pocket.
- Step A: Chrome's `device_presence` row was deleted when the tab went background (visibilitychange fired DeviceHidden). Its **EventSource stays open**, so it counts toward the live SSE-connection gate and the engine runs the PresenceCheck. Chrome pongs `is_active: false` (`hasFocus()` is false). The suspended iOS PWA has its EventSource closed and never pongs. No `is_active` pong arrives → `push_allowed = true`.
- Step B for Chrome: row 4 → OS push fires; the bell badge updates via SSE when the page regains focus.
- Step B for iOS: row 5 → OS push fires via APNs.
- Both devices get a push.

If this scenario shows "Chrome bell only, no OS push", the bug is one of these. Tests in §5 cover each.
- (a) The visibilitychange event didn't fire DeviceHidden, leaving Chrome's presence row as a stale candidate.
- (b) Chrome was never subscribed to push (no permission grant, no service worker registration).
- (c) The push was sent but Chrome dropped it.

## §3: The PresenceCheck protocol

**Trigger.** When `NotificationCreated` is emitted, the engine decides whether to run the PresenceCheck from two signals (`expected_pong_count` in `crates/lucidos-engine/src/scheduler/push.rs`):

1. **Live SSE-connection count** (`engine.sse_connections.count()`, the number of open `GET /api/v1/events` streams). This is the **primary, authoritative** gate: a connected page can pong however stale its heartbeat is. It increments when a page opens its EventSource and decrements when the stream drops, so it can't go stale.
2. **`device_presence` candidates**: rows fresher than `PRESENCE_STALE_AFTER` (120s, `crates/lucidos-engine/src/core/device_presence.rs`). A 30s heartbeat (`device-presence.ts:HEARTBEAT_INTERVAL_MS`) refreshes them, plus a forced refresh on `visibilitychange-while-visible`. This secondary signal covers a page that heartbeated recently but whose SSE connection just dropped (network blip).

A `DeviceHidden` deletes the device's row and stamps `devices.last_seen_at` with the hide time. So the agent's Known devices list still reads a hidden device as seen at the hide, not at its last page load. The stamp plays no part in this gate. `navigate_ui` reads the same row to tell the agent whether its target device had Lucidos visible.

The engine runs the PresenceCheck when `max(sse_connections, candidate_count) > 0`, waiting for that many pongs (the deadline short-circuits once they all arrive). It sets `push_allowed = true` directly **only when both are zero**: nobody is reachable (the "phone in your pocket" case).

**A connection owes exactly one pong, and that is a client-side contract.** The count is connections, not documents, so everything sharing a connection answers once. The *shared SSE holder* does that by ORing its documents' answers (§ Multi-tab above). Two consequences show up in traces:

- An app frame has **no presence voice**. The shell around it relays its frames off the shell's own connection, and the frame never answers. An app in its own tab attaches to the holder and never answers either. So a workspace with only app tabs open, and no shell anywhere, pongs nothing and takes the OS push.
- A connection whose documents all decline to answer produces no pong. The engine waits out `DEADLINE_MS` and pushes, which is correct: there was no shell to show a toast.

**Why the SSE gate, not heartbeat freshness alone.** iOS suspends the 30s heartbeat timer while a PWA is foregrounded, and never fires `visibilitychange`. So an active page's `device_presence` row ages past 120s while its EventSource stays open and would pong `is_active`. A heartbeat-only gate skipped the PresenceCheck and pushed on top of the active page. The SSE count doesn't depend on the heartbeat, so the active page always gets to pong.

**Ping.** The engine broadcasts a transient SSE event to every connected page. It is a **pure pong trigger** and carries no toast content (that rides on `NotificationToastRequested`, §4):

```json
{
  "type": "PresenceCheck",
  "data": {
    "notification_id": "<uuid>",
    "event_id": "<uuid or omitted>",
    "deadline_ms": 2000,
    "sent_at_ms": 1700000000000
  }
}
```

The deadline (`DEADLINE_MS` in `crates/lucidos-engine/src/scheduler/push.rs`) is sized for an iOS PWA reaching the engine over cellular or a Tailscale tunnel. Real traces show two RTT bands for the SSE-out plus pong-POST trip:
- Steady-state cellular or Tailscale relay: 400–800 ms.
- The first packet after the phone radio resumes from idle pays the Tailscale (userspace WireGuard) path renegotiation: 1100–1800 ms.

The deadline governs how long the engine waits before deciding, but it **does not race the toast**. A slow pong degrades to "OS push instead of toast", never "toast AND push" (§4). `notify_one` wakes the engine once every expected device has answered (see `run_presence_check`). So a larger deadline costs latency only when a `device_presence` row is stale (page killed without firing `DeviceHidden`).

`event_id` rides along only so the pong can report `event_in_viewport`. `sent_at_ms` is the engine's wall clock at emit. The page drops a PresenceCheck where `Date.now() - sent_at_ms` exceeds `deadline_ms + grace` ("Freshness gate" below). A dropped PresenceCheck only skips a pong the engine would have discarded anyway.

**Freshness gate.** iOS PWAs suspend JS in the background and queue SSE messages until the page is visible again. When the user taps the OS push, the queued PresenceCheck flushes long after the deadline. The page compares `Date.now() - sent_at_ms` against `deadline_ms + 2000ms` (`STALE_GRACE_MS` in `presence-pong.ts`) and drops late pings without a pong. It reads `deadline_ms` off the payload, so an engine bump flows through automatically. The *toast's* own staleness gate is separate, on `NotificationToastRequested` (§4).

**Pong.** Every receiving page synchronously gathers its live state and POSTs `/api/v1/presence-pong`:

```json
{
  "notification_id": "<uuid>",
  "device_id": "<string>",
  "is_active": true,
  "focused_thread_id": "<uuid or null>",
  "event_in_viewport": true
}
```

- `is_active` is `isPageActive()`: visibilityState + hasFocus on desktop, visibilityState only on iOS PWA.
- `focused_thread_id` is the current value of the `focusedThreadId` signal.
- `event_in_viewport` is computed by the page. It locates the source event in the DOM (`document.querySelector('[data-event-id="..."]')`). If absent (virtualized list, not rendered) → `false`; if present → an `IntersectionObserver` / `getBoundingClientRect()` check.
  - The rect is intersected against the **transcript scroll container's** band, not the window's. The app header and the prompt region inset the transcript. An event behind either strip is inside `window.innerHeight` yet hidden, and calling it visible would suppress a needed notification.

**Decision.** After `deadline_ms`, the engine has zero or more pongs.
- `push_allowed = !any(pong.is_active for pong in pongs)`. Pongs are grouped by `device_id` and OR'd within a device (see §2 "Multi-tab"); the OR across devices is the same operation.
- If every candidate device ponged before the deadline, the engine decides immediately.
- **`push_allowed = false` → the engine emits a `NotificationToastRequested` SSE** (`emit_toast_requested` in `push.rs`) and sends NO push.
- **`push_allowed = true` → the engine fans out the OS push** and emits no toast event.
- So a notification produces a toast-request OR a push, never both. The two hang off opposite sides of one decision rather than racing (§4).

**No batching across notifications.** A burst of 10 notifications in 5 seconds produces 10 independent PresenceChecks. Each spawns its own deadline timer and they run in parallel. So burst latency is one deadline window (~2 s in the worst-case "stale candidate" branch), not 10 × `DEADLINE_MS`. Batching is a future optimization; the spec does not depend on it.

**Failure handling.**
- A device that got the ping but failed to POST a pong → treated as not-active. The page is likely dying or the network unreliable, and pushing is the safer side.
- A pong arriving AFTER the deadline → discarded with a `200 OK` ack (not a `409`; this race is normal).
- A pong with an unknown `notification_id` → `404`. The notification was already decided and forgotten.

**Why this works without silent-push risk.** The engine decides server-side BEFORE sending the push. If it sends, the service worker always calls `showNotification()`, so no browser sees a push the SW swallows. The `userVisibleOnly: true` subscription contract stays clean, and the browser never penalises the origin for "ghost" pushes.

**Cost.** Up to one `DEADLINE_MS` (currently 2 s) of added latency when a reachable device fails to pong. Two cases produce a non-ponging but counted device:
- A `device_presence` row still fresh because the page was killed without firing `DeviceHidden`.
- An SSE connection open at the socket level whose page can't run JS (a heavily throttled or just-suspended background tab).

When every expected device pongs, `notify_one` wakes the engine at the last pong, so there is no latency tax. An *active* page pongs within one round-trip, so the common "suppress and toast" path is fast. With nobody reachable (the phone-in-your-pocket case), there is no delay at all.

## §4: The in-app surface (page-local)

The bell badge and toast are **two distinct triggers** that decode the same notification:

1. **Bell badge**: driven by SSE `NotificationCreated`. The handler, `handleNotificationSSE` in `notifications.ts`, reloads the **unread set** (`unreadNotifications` in `store.ts`). It reads `GET /api/v1/notifications?filter=unread`, capped at the API's 100. It fires on every connected page, active or hidden, so the badge stays in sync.
   - **The badge count is DERIVED, not fetched.** `unreadCount` is a `computed` over `unreadNotifications.length`, so no separate count can drift from the notifications (bell showing unread over an empty inbox).
   - **The reload is sequence-guarded.** Every async load (`loadUnreadNotifications`) claims a monotonic seq and applies only if still the latest.
   - Every local optimistic mutation (`markReadOptimistic` / `navigateToNotification` / `markAllRead`) invalidates any in-flight load before it changes the set.
   - That covers **in-app auto-read** (Row 1): a page fires a created-reload, then marks the row read. The optimistic removal invalidates the reload, so the notification can't flash on the badge.
   - Page reload hydrates the set fresh from the same endpoint, and there is no cached count.
2. **Toast**: driven by SSE `NotificationToastRequested` (`handleNotificationToastRequested` in `in-app-notification-toast.ts`). The engine emits it **only after deciding to suppress the OS push** (§3 "Decision": `push_allowed = false`). It carries `title` / `body` / `thread_id` / `event_id` / `app_id` / `tap` / `sent_at_ms`. Active pages render the toast (or auto-read on Row 1); hidden pages ignore it (Row 4). Inactive devices get the OS push instead, never both.

   The toast once fired on the §3 PresenceCheck pong handler, which raced the push: on a slow iOS link the push landed on top of the toast. Behind the engine's decision, a slow pong degrades to "OS push, no toast".

**A producer that names a source event gets a `navigate` tap without asking for one.** `default_tap` (`scheduler/notifications.rs`) is what an omitted `tap` resolves to. Both producer entry points use it: the `send_notification` LLM tool and `POST /api/v1/notifications`. It answers `Navigate` to the notification's own `thread_id` and `event_id` when BOTH are present, `Modal` otherwise. An explicit `tap` always wins, including an explicit `{"kind":"modal"}`, and nothing is backfilled.

**Both entry points refuse an event the linked thread does not hold** (`verify_event_anchors`, beside `default_tap`). The page looks for an anchor only inside the thread it opens, so any other event makes a dead tap. The check covers two pairs, each on its own: the row's `thread_id` with its `event_id`, and a thread tap's `to.id` with its `to.event_id`. It refuses an id that names no event, an event in another thread, and a workspace domain event, which lives in no thread. The tool returns an error the agent can act on, and the HTTP route returns a 400. (A trigger fired by a domain event once passed that event's id, and the reader met "That event is not shown in this thread".)

Why `event_id` and not `thread_id`? An `event_id` is the producer naming a specific thing to look at: a question, a permission request, a failure card. The destination is the point, and a card in between is a wasted tap. A `thread_id` ALONE is deliberately not enough. The engine stamps one on every `send_notification` from the origin thread, so it is provenance rather than intent. Navigating on it would drop a daily-summary notification into the middle of that trigger's own agent transcript.

**A tap anywhere on the toast opens it, and the X defers it. There is no button, no auto-open and no `[OK]`.** For `tap.kind === 'modal'` (the default, or a missing `tap`) and `tap.kind === 'navigate'`, the Rows 2/3 toast opens on a tap anywhere on the card (its `onClick`). The tap runs `dispatchDeepLink`. For `modal` it opens the notification detail in the content pane (the `view-notification` action); for `navigate` it goes to the destination. It also marks the notification read.

The toast **persists** (no auto-dismiss). The close (X) button dismisses **without** marking read, so the row stays unread in the bell badge and panel. There is deliberately **no `[OK]` / acknowledge button**: marking read *without* opening would bury an unanswered question. The detail panel **never auto-opens** when the toast appears (`notification-toast-requested.test.ts` pins this). No toast moves keyboard focus, so an unsolicited one cannot steal focus mid-typing and pre-arm a reflexive Enter.

`view-notification` opens the detail in the content pane (`panelOverlay = { type: 'notification-detail' }`, rendered by `NotificationDetailInline`), not a modal overlay. `tap.kind === 'modal'` is the wire name of the tap, not a UI modal.

**A toast lives exactly as long as its row is unread.** The bell badge, the Unread tab and the toast are three projections of one set (`unreadNotifications`), so a read leaves a toast nothing backs. Two inputs enforce it, and they answer different questions.

**The unread-set watch answers a read this page can see.** `installNotificationToastLifetime` (`in-app-notification-toast.ts`, wired from `store/effects.ts`) subscribes the toasts to that set and removes the toast of every row that leaves it. It is the fast half, answering on the tick the reader acts, because that drop is optimistic and local. It covers the **seen target** rule below, the Notifications panel, the detail chevrons, and Mark all read. Three properties of it are deliberate:

- **It fires on the TRANSITION, never on bare absence**: the ids the set held a moment ago, minus the ones it holds now. A toast rides `NotificationToastRequested` while the reload `NotificationCreated` kicked off is still in flight, so a brand-new row reads as absent too. Dropping on absence would hide a notification nobody has seen.
- **It stands down while the set is not loaded**, holding its baseline, so a reconnect or a workspace switch clears no live toast.
- **It removes structurally** (`removeToast`, not `dismissToast`), because the row was read rather than deferred by the reader.

**The `NotificationRead` arm answers a read this page cannot see.** It is the authoritative half and is slower by a round trip. It drops the toast by id from the `handleGlobalEvent` arm in `thread-sync.ts`, and `NotificationsAllRead` clears the lot. Two cases need it, and the transition above reaches neither:

- A read made on **another device**.
- A read landing before the reload that would have carried its row. The id was never in the set to leave it.

The engine emits `NotificationRead` only on a real unread-to-read flip, so a redundant write cannot double it. The **overflow** toast follows the same rule by tracking the ids it folded: a read decrements its count, and the last one removes it. Its pile is authoritative only while it is showing, so a fold arriving after the reader cleared it starts again at `+1`.

**The inbox row runs the same router, so all four surfaces agree.** A row in the Notifications list dispatches through `dispatchDeepLink`, exactly as a tap on the toast and the two OS taps do. The mapping is `parseDeepLinkFromInboxRow` in `store/actions/notification-deeplink.ts`. A `navigate` row reaches its destination in one tap, and a `modal` row opens the detail. A jumping row leaves the notification's own text unseen, so it grows a trailing chevron that opens the detail.

A `modal` row gets no chevron, since its body already opens the detail. Two shapes are load-bearing. `.notification-item` must stay a real `<button>`: the e2e helpers dispatch a synthetic `el.click()` on whatever matches, and that bubbles up rather than down. The chevron is therefore a SIBLING of it in a flex row, since a button cannot contain a button. The row's dim, unread tint and hover moved onto the `.notification-row` wrapper, so they span the chevron too.

**A notification that reaches no thread offers Discuss, and the detail's action row is therefore never empty.** A row reaches a thread two ways: its own `thread_id` column, or a thread-targeted `navigate` tap, which the panel also labels "Open thread". Either way that thread IS the discussion, so Discuss stands down and the two never appear together.

Everything else gets **Discuss**, which starts a Lucidos Agent thread with the notification quoted and SENDS it (`store/actions/notification-discuss.ts`, over `sendSeededPrompt`). The message is ordinary text on the ordinary send path, so what the agent reads is what the transcript shows. `ensureFocusedComposeThread` allocates the thread id client-side, so Discuss reveals the thread pane before the request goes out rather than after it. It leaves the notification detail open behind the conversation, so the reader keeps their place in the inbox. One tap is the whole gesture, with a single exception: a draft already in progress raises a confirm first, because a click must never blow away typed text.

**Opening a notification never waits on the network for data the page already has.** Every open gesture (an inbox row, a toast, or an OS push tap) goes through `dispatchDeepLink` to one of the two branches below. Both must put something on screen on the tap itself, not after a round-trip.

Breaking this rule is invisible in development. On an iOS PWA over Tailscale one round-trip takes 400 to 800 ms, and 1100 to 1800 ms after the radio resumes (§3). A tap that changes nothing for that long reads as dead.

- **`tap.kind === 'modal'` (the notification detail).** `viewNotification` resolves the row from whichever loaded list holds it (the browse list `notifications` or the unread set `unreadNotifications`) and opens synchronously.
  - This is sound because both lists carry WHOLE notifications. `NotificationStore::get_filtered` and `get_by_id` select an identical column list and serialize the same `Notification`.
  - Only a genuine miss fetches: in practice the cold push-tap deep link, since a warm page has loaded the unread set for its badge. The fetch reveals the pane at once with `NotificationDetailInline`'s skeleton, delay-gated past `SPINNER_DELAY_MS`.
  - The `panelOverlay` is written only with a real notification in hand, so a failed fetch leaves no phantom nav-stack entry. The pending id lives beside it in `notificationDetailPending`.
- **`tap.kind === 'navigate'` to a thread** (the common case by far: 80% of notifications in a mature workspace). `focusThreadOrBootstrapResult` focuses synchronously when the thread is in `threadMap`.
  - Otherwise it focuses **optimistically** and reveals the thread pane before fetching metadata, so `ThreadView` renders its delay-gated skeleton instead of a dead interval.
  - `bootstrappingThreadId` exempts that thread from ThreadView's stale-pointer cleanup, which would unfocus a thread absent from the map. A bootstrap that does not land restores the prior focus.
  - The metadata comes from `GET /api/v1/threads/:id`, never the grouped `GET /api/v1/threads`. The grouped call assembles saved, recent archive, active, composing and family base, costing hundreds of milliseconds on a large workspace.
  - A cold push tap always meets an empty map, because the deep link dispatches while `loadAllThreads` is in flight. So this path runs on every such tap.

**A thread-targeted tap is refused at the PRODUCER unless its id names a thread.** A notification is written and pushed in one stroke. So a tap the page cannot resolve is a dead deep link by the time anyone sees it, read as `Thread "<id>" no longer exists`. Every producer settles `tap.to.id` first, through `notifications::resolve_thread_tap_id`, and `create_notification` re-runs it as the chokepoint backstop. A missing id is refused too: the router's own "you forgot the id" would reach a banner nobody can repair.

`send_notification` resolves the `current` / `this` alias to **the thread the notification is about**, which is the `link_thread` its own `thread_id` column carries. Usually that is the calling thread. Where a trigger fired on another thread's event, it is the ORIGIN thread instead. So the row's two buttons agree, and the tap never lands in the trigger's own transcript. `navigate_ui` has no such notion and resolves to its calling thread. `POST /api/v1/notifications` and the SDK's `POST /api/v1/ui/navigate` pass no caller, so the alias is refused there with every other non-uuid: neither has a thread of its own.

Reading is deliberately unguarded. `Tap`'s decode still accepts a non-uuid id, because rows written before the guard have to stay readable to be repaired. The migration `20260918051213_repair_alias_notification_tap_thread_ids.sql` did that, from each row's own `thread_id`.

**Where a `navigate` tap LANDS inside the thread, and what happens when it can't.** The *source event* is resolved in the DOM by `[data-event-id]` (`scrollToEventAndPulse` in `components/chat/scrollState.ts`), scrolled to, and left carrying the *navigation focus marker*. Three rules govern which element that is:

- **Every deep-linkable event is addressable, whether or not it starts a turn.** An event that begins an exchange (`UserQuestionAsked`, `CodingAgentPermissionRequest`, `CommandPermissionRequested`, `McpConsentRequested`, …) stamps the whole turn, and the pulse narrows to its `.initiator-panel`. An event folded into a turn as a STEP stamps the card that renders it instead, and the pulse stays on that card. Today that is `ResponseFailed`, whose failure card is what the user is being sent to see. Landing on the whole turn would bury the failure in a long coding-agent run. The same `[data-event-id]` lookup backs `event_in_viewport` below, so Row 1's auto-mark-read now works for those events too.
- **An event that stamps nothing is reached through the turn that CONTAINS it.** The two stamps above are the whole set. `stampedEventIds` in `store/thread-events/exchange-render.ts` declares it. A source-scan tripwire in `deep-link-anchor.test.ts` fails if `ChatExchange` grows a third without declaring it. So every other event, such as an ordinary tool call or a `CodingAgentIdled`, is addressable only via its turn. `deepLinkAnchorForEvent` does that re-targeting: the event itself when it stamps its own element, its exchange's starter otherwise.

  Every event deep link uses it, through `landOnEvent` (`store/actions/threads.ts`). The engine makes sure a notification's event lives in the thread it opens (`verify_event_anchors`, above). It does not make sure the event stamps its own element. A form request raised mid-turn, such as a `CredentialRequested`, is a step, so its tap lands on the turn holding it. The anchor is asked again on every look, because a cold tap only knows it once the thread's events arrive.

  The *event wait* step's **show it** goes further, because a wait can match ANY event type in ANY thread. In practice it matches a `CodingAgentIdled` in whichever thread it was watching. That step resolves the owning thread first, through `GET /api/v1/events/:event_id/location`. It answers `thread_id: null` for a workspace domain event, and 404 for an event id with no row. The step then re-targets, and hands off to the same `focusThread(threadId, { targetEventId })` path a notification tap uses.
- **A fullscreen app panel is the whole viewport, so a thread tap leaves it.** `handleNavigationRequest`'s `thread` branch calls `exitAppFullscreen()` before it focuses, exactly as `new-chat` does. Only fullscreen goes, and `panelOverlay` stays. The split gives the conversation its own pane, so closing the app would cost the reader what they were working in.
- **A target with no BOX is waited for, and every link says how it ended.** A collapsed Conversation pane zeroes the target's own rect, and an ancestor clipped to nothing shows none of a target that still measures full size. `isElementVisible` rejects both, and gaining a box is a layout change no `MutationObserver` can see. So the link watches that box with a `ResizeObserver` too (`watchForABox`), inside its existing deadline. Whatever the ending, it writes one `[Client/deeplink] outcome` line to `engine.log`. That line names the ending, how many elements matched, how many had a box, and where the transcript was going to rest.
- **A target that never renders is reported, not silently dropped.** The resolve waits out a deadline (4s) for a lazily-loading thread. If the event is still missing, a warning toast says the event is not shown in this thread. No pulse, since there is no element to mark, and **no scroll**: the transcript stays where it was. The bottom of the thread is not the place the tap asked for (the *reading position* entry in `docs/glossary.md` has the wider rule). The Changes-panel deep link (`data-change-id`) shares the same deadline and reports the same way.

**The notification's `title` is the toast's title, and a producer must never restate it in the `body`.** `showInAppNotificationToast` passes the two straight through: `showToast(body, 'info', { title })`. The title is the bold line. The body is plain text under it: a newline is a line break, and a `"• "` line is shown as written. A notification with no body shows its title alone, not bold.

So write the body as *content only*: `"• "` lines for a list, a bare sentence for a single item. The toast, the detail pane's `<h2>` and the OS push banner each show the title already, so a body that repeats it shows it twice.

**The body may be markdown, and only the detail pane renders it.** The toast, the macOS banner and the web push show text as written. So the engine sends them a plain-text copy (`plain_text_body` in `scheduler/notification_plain_text.rs`). Markup goes, a list item starts with `"• "`, and a link keeps its label. The notification row keeps the markdown. A body that forwards a question card therefore reads cleanly on every surface.

**A body that names a Settings page links it as `settings:<view>`**, with the same view id as `navigate_ui settings_view`: `[Settings → System → Backup](settings:backup)`. The label is the breadcrumb route, so every plain-text surface still reads the route. The engine's backup and disk-space notifications build theirs from one `SettingsPage` value (`scheduler/notifications.rs`). That value also builds the tap, so the link and the tap open the same page.

**The passive `tap: "none"` kind is RETIRED: every notification is openable.** It was a button-less variant that auto-marked-read on show and auto-dismissed after 5 s. It went so a notification never renders as a dead banner (see `docs/plans/2026-07-02-remove-notification-tap-none.md`). Readers can still meet old `none` data, handled like this:
- Nothing produces `none`. The `send_notification` tool schema and the SDK `Tap` type don't offer it, and the DB `notifications_tap_valid` CHECK rejects it for new writes.
- A migration rewrote existing `{kind:none}` rows to `{kind:modal}`.
- Historical `NotificationCreated` events keep `{kind:none}` forever (event-sourcing immutability). The engine's custom `Tap` deserialize coerces them to `Modal` on replay, so a projection rebuild never re-emits a `none` row.
- The frontend `resolveDeepLink` demotes any stray `none` (an old SW message or stale URL) to the openable `view-notification` path. It is never a passive `mark-read` or a no-op.
- Row 1 (auto-read because the user is looking at the source event) applies to every kind.

The toast's only trigger is the `NotificationToastRequested` SSE, which fires only when the push did not. **Why not fire it on `NotificationCreated`?** iOS PWAs queue SSE messages while backgrounded. After an OS push tap, the queued `NotificationCreated` flushes once the user has landed, so `isPageActive()` is `true`. The matrix would then pick Row 2/3 and add a duplicate toast. `NotificationToastRequested` has its own freshness gate (`TOAST_REQUEST_STALE_AFTER_MS` in `in-app-notification-toast.ts`), so a late flush never pops a toast on a resume.

**On `PresenceCheck` received via SSE (and not dropped by the §3 freshness gate), the page only pongs:**

```ts
submitPong(notification_id, {
  device_id,
  is_active: shellIsActive(),
  focused_thread_id: focusedThreadId.value,
  event_in_viewport: payload.event_id ? isInViewport(payload.event_id) : false,
});
// No toast here: that's the engine's call, delivered via NotificationToastRequested.
// submitPong goes through the TRANSPORT: direct connection POSTs at once, a
// shared one hands the answer to the holder to OR with its peers.
```

**`is_active` means the SHELL is what the user is looking at, not merely that the page is visible.** `shellIsActive()` (`store/actions/presence-pong.ts`) is `isPageActive()` AND no app fullscreen, covering both the native mode (`appFullscreenHost`) and the iOS CSS mode (`appPseudoFullscreen`).

An app filling the screen therefore takes the **OS push**, not the toast. Two reasons:

- **Only the shell can show a toast**, so presence answers "can we reach this person without pushing?". A reader deep in a fullscreen app is not reading the shell.
- **It makes fullscreen agree with a popped-out app window.** The two are identical from where the user sits, and a popped-out one has always taken the push.

An app merely open in the content pane is unaffected: the shell around it renders the toast. The *device-presence heartbeat* is deliberately unaffected too. It reports whether the page is visible, which under a fullscreen app it still is, and the engine reads it only to count expected pongs.

**On `NotificationToastRequested` received via SSE (and fresh per `TOAST_REQUEST_STALE_AFTER_MS`), the page applies the §4 row matrix:**

```ts
if (!isPageActive()) return;                       // Row 4: hidden, bell badge only.
const onSource = payload.event_id
  && focusedThreadId.value === payload.thread_id
  && isInViewport(payload.event_id);
if (onSource) {
  markReadOptimistic(payload.notification_id);     // Row 1: looking at the source event.
  // no toast
} else if (currentNotificationToasts()) {
  showToast(payload);                              // Row 2 / 3: active, scrolled away or other thread.
}
```

**The Rows 2/3 toast is switchable: `notification_toasts`, a global preference, default `true`.** Set it to `false` and the toast never renders, the `+N more` overflow toast included. The notification still counts on the bell badge and waits in the Notifications panel, the deferred queue the switch sends it to. Unlike `push_notifications` this is workspace-wide, so one write covers every device. Settings → Appearance & Behavior → Notifications carries the row as **In-app toasts**.

**No OS push arrives in its place.** The preference does not change presence, so the engine's `push_allowed` decision (§2, §3) is untouched and a present device stays out of the fan-out. The user asked not to be interrupted, not to be interrupted differently. The gate sits in `showInAppNotificationToast` **below** the Row 1 branch, so reading the source event still auto-marks it read. It is deliberately not in `showToast`, which also serves toasts answering the user's own action (an apply-change result, an error).

**Outcomes across devices:**
- All devices hidden → push fans out to all subscriptions. No in-app toast anywhere.
- One device active, others hidden → push suppressed globally; in-app toast on the active device only.
- Multiple devices active → push suppressed globally; in-app toast on every active device whose §4 row resolves to toast.
- Active device pongs as Row 1 (event in viewport) → no push to anyone, no toast on this device (auto-read), toast on any OTHER active device on a different thread.

**Reconnection.** Page load hydrates the unread set from `GET /api/v1/notifications?filter=unread`, so a notification that arrived offline shows in the badge. Toasts missed while offline are NOT replayed: the unread set is the durable signal.

**Read marks are global.** Reading a notification on one device clears the badge on all others via the `NotificationRead` broadcast. Auto-read in Row 1 emits the same event as a manual read.

**Cross-device OS-banner dismiss (macOS desktop only).** A read also removes the *already-delivered OS banner* on other devices, but only on the native macOS desktop client.
- When a read flips (`NotificationRead`, or `NotificationsAllRead` for mark-all), the engine broadcasts a transient `NativePushDismissRequested`. It carries `notification_id` (`Some(id)` for one, `None` for all) and `sent_at_ms`.
- A connected Tauri desktop app removes the matching delivered banners via `UNUserNotificationCenter.removeDeliveredNotifications(withIdentifiers:)`. It drops the stashed deep link so a phantom tap can't route (see the native section below).
- A `None` removes every banner **the reading workspace raised**, never another workspace's.
- The page handler `handleNativePushDismiss` gates on `isTauri()` (browser / PWA ignore the event) and on the `sent_at_ms` freshness budget. That budget bounds, but does not close, the window where a late dismiss-all clears a banner created after an all-read.
- There is **no** `isPageActive()` gate, because removal is a harmless no-op when nothing matches.

This is the **one platform where cross-device dismiss is both possible and deterministic**: the desktop app stays SSE-connected, and native notifications have no "must show something" rule. The **open web cannot** do it. Safari revokes a Web Push subscription after 3 silent pushes, and Chrome/Firefox show a default "site updated in background" banner on a silent push. So browser / PWA banners persist until swiped, while the in-app badge still syncs via `NotificationRead`.

A **native iOS app** could dismiss via the same UN API. That is deferred and best-effort, since a backgrounded iOS app's SSE is suspended and would need a silent APNs push: see `docs/plans/2026-06-19-ios-native-apns-app.md`. The abandoned web-only approach (a visible "✓ Read on another device" tombstone) is in `docs/plans/2026-05-18-cross-device-notification-dismiss-design.md`.

### A notification also clears once you have looked at what it points at

Row 1 above is a one-shot. It runs on the SSE that announces the notification and never again. Reaching the same event a minute later left the row unread, owing the Notifications panel a second read. The **seen target** rule (`store/actions/seen-target.ts`) makes the same test standing. It is what clears a row reached from the drawer's Blocked list, or by foregrounding an app that already had the thread open.

**Seen means the same thing Row 1 means.** A tap naming an event is seen when that event's own card is in the transcript's visible band. The measure is the same `isInViewport` the pong uses. A tap naming a place with no card is seen when that place is on screen: an app, a file, a trigger, a settings sub-section, a panel. The pane showing it must be the current mobile pane, or a desktop pane the split gives real width. A desktop split puts a thread and a panel in front of the reader at once, so both count.

**It must hold for `SEEN_DWELL_MS` (1000 ms) with the page active.** A glimpse is not a read. Five things put a target briefly in front of a reader who never asked for it:

- Drawer browsing with the arrow keys, which moves the focused thread per keypress.
- A mobile swipe, which passes through the middle pane.
- The focus hand-off after an archive.
- A deep-link bootstrap, which focuses before its fetch lands.
- A fast scroll.

Leaving cancels the wait rather than pausing it, so time in the band has to be continuous. A background cancels it too, and the paired wake starts it over.

**The matrix above is untouched, and the strict measure is why.** This rule's condition IS Row 1's condition, so a notification arriving while its card is on screen was already being auto-read. Row 2 is the case a looser "the thread is open" reading would have broken: on the source thread with the event scrolled away, the card is out of band, so this rule stays silent and the toast still fires. Rows 3, 4 and 5 name places the card cannot be visible in. Row 1 keeps its own code because it does a second job this rule does not, suppressing the toast.

**A `modal` tap is never cleared this way**, since its place is the notification detail and opening that already marks it read. Neither is a notification matched on its own `thread_id`: that column is provenance, not a destination (see "Why `event_id` and not `thread_id`?" above), so reading a trigger's own transcript must not clear the daily summary it produced there. Only `tap.to` names a target. The rule marks read and nothing else: it never scrolls, never archives, and never clears the Blocked state. What the read then clears is not its to decide, and the row's toast goes with its badge under the lifetime rule above.

**A *read request* rides the same watch, with the same dwell** (ADR 0409). It points at no event, so its target is the end of the thread's newest reply: the bottom edge of the last turn in the visible band. It counts only once the turn has ended, and only on the thread the reader has open.

Seeing it posts `POST /api/v1/threads/:thread_id/read-request/seen` with the summary version the page holds. The engine's `ThreadReplySeen` then clears the request on every device. If the thread changed since, the post answers 409 and the next sample reports again. It has no notification row and touches none.

One set of constructors (`store/actions/visitKeys.ts`) spells both the place a tap names and the place the shell is showing, so the two cannot drift apart. Four navigate targets name no revisitable place and are excluded: `url`, `new-chat`, `new-app` and `new-trigger`. A test walks the generated `NAVIGATE_TARGETS` so a new one has to say which it is.

### Native desktop OS surface (Tauri)

The Tauri desktop app embeds a WKWebView, which has no service worker and can't subscribe to Web Push. The native banner fills that gap, driven page-side from the same SSE stream.

**This whole surface needs the client process to exist.** The engine records notifications with no client running, but the client shows the banner (`show_native_notification`). So the packaged build installs a *login agent* (`com.lucidos.client`, see `docs/desktop-app.md`) beside the always-on service, and a restarted Mac still gets banners. A login-started client is menu-bar-only with a hidden window (the trayed state below), so it takes the native-banner branch.

- On the **push-allowed branch** (`push_allowed = true`, §3 "Decision") the engine emits a `NativePushRequested` SSE *alongside* the web-push fan-out. It carries the same content as `NotificationToastRequested` (`title` / `body` / `thread_id` / `event_id` / `app_id` / `tap` / `sent_at_ms`).
- The page handler (`handleNativePushRequested` in `native-push.ts`) gates before touching the OS:
  - **not Tauri → ignore**: browser / PWA already got the real web push on this branch;
  - **stale per `NATIVE_PUSH_STALE_AFTER_MS` → ignore**: a late SSE-queue flush, as with the toast's freshness gate;
  - **`isPageActive()` → ignore**: the OS surface is for non-active devices, and macOS suppresses banners for the frontmost app anyway.

  It then invokes the app's `show_native_notification` command (`utils/tauri.ts`). It passes the deep-link target in the SW-message shape (`notification_id` / `thread_id` / `event_id` / `tap`).
  - **The trayed-window case depends on the native-active bridge (§1).** Without it, a window trayed by macOS `orderOut:` keeps reporting `visibilityState='visible'` / `hasFocus()=true`. The engine then suppresses the push and this handler early-returns, leaving only the bell badge. With the bridge, a trayed or unfocused client reports inactive and gets the native banner. A window behind another app already reports inactive via `hasFocus()`, but only the bridge fixes the trayed case.
  - **The bridge is SEEDED on page load, not just transition-driven.** The `native-window-active` cache (`utils/nativeWindow.ts`) defaults to `true`. Otherwise only the transition events Rust's `on_window_event` emits update it (`Focused` / `CloseRequested` / `show_main_window`).
  - Tauri does **not** replay those events to a late listener. A page reloaded while backgrounded or trayed would keep the `true` default and get its push suppressed into an invisible toast. That covers a crash-watchdog reload (`lib.rs`), the "New version → Refresh" reload, and a cold or unfocused launch.
  - So `startNativeWindowActiveTracking` first **pulls the authoritative AppKit state** via the `get_native_window_active` command (`focused && visible && !minimized`). Any read failure → `false`, the safe direction. It seeds the cache **before** registering the transition listener. Off-Tauri it's a no-op, and the cache stays `true`.
    - **The seed reads the CALLING page's own window, not `main`.** Tauri injects the calling `tauri::Window` into the command, as `focus_calling_window` and `window_ready_to_show` do. Reading `main` let a backgrounded second window seed itself **active** off a focused first one.
- **Why a direct `objc2` `UNUserNotificationCenter` path, not a crate.** Two crates were tried and dropped.
  - `tauri-plugin-notification`'s desktop `show()` is fire-and-forget and never reports the click (its `onAction` is mobile-only), so it can't route taps.
  - `mac-notification-sys` (what `notify-rust` and the plugin use on macOS) drives Apple's **deprecated `NSUserNotification` API**. That API no longer delivers on recent macOS (silent on macOS 26 "Tahoe").
  - The shipped path drives Apple's **`UserNotifications`** framework through `objc2`. It builds a `UNMutableNotificationContent`, posts a `UNNotificationRequest` (no trigger = deliver now), and installs a `UNUserNotificationCenterDelegate` to capture taps. See `crates/lucidos-app/src/notifications.rs`, which the `show_native_notification` command forwards to.
- **The delegate implements `willPresent`, so banners show even if the app is frontmost.** macOS suppresses a frontmost app's banner unless the delegate opts in via `userNotificationCenter:willPresentNotification:`. Without it, a stale-false active signal makes `show()` fire while Lucidos is focused, and the banner silently drops. The delegate returns `[.banner, .list, .sound]`. This never double-surfaces: `NativePushRequested` and `NotificationToastRequested` sit on opposite branches of the one `push_allowed` decision.
- **Cross-device dismiss (removal counterpart).** The same `objc2` path also *removes* delivered banners. The `dismiss_native_notification` command (`notifications::dismiss`) calls `removeDeliveredNotifications(withIdentifiers:)`.
  - It removes one banner, or every banner the calling workspace raised (enumerated via `getDeliveredNotifications`). The engine's transient `NativePushDismissRequested` SSE drives it (see §4 "Cross-device OS-banner dismiss").
  - It also drops the banner's stashed deep link from the in-process map, so a removed banner can't route.
  - Both arms take the workspace (see "A banner belongs to the workspace that raised it" below). `handleNativePushDismiss` gates on `isTauri()` + `sent_at_ms` freshness, with no `isPageActive()` gate. Inert in `tauri dev` / off macOS, like `show`.

**Enabling.** Desktop has no web-push subscription to create, and the macOS `UserNotifications` path has no JS-queryable permission. macOS shows banners per the app's authorization, requested once at startup (`notifications::setup`) and changeable in System Settings → Notifications. So "enabling push" on the Tauri app is a no-op success. That is the same `initPushSubscription` entry point the LLM `enable_push_notifications` tool and the settings toggle drive.

**Native banners require a packaged `.app` build.** `UNUserNotificationCenter.currentNotificationCenter()` throws for a process with no bundle identifier. A `tauri dev` build is an unbundled `cargo run` binary, so `notifications::{setup,show}` both short-circuit on `tauri::is_dev()`. Native banners are inert in dev *on any macOS version*, and the dev notification channel is the browser's web push. A packaged build (`cargo tauri build`) uses the app's bundle identifier (`app.config().identifier`) and delivers. macOS prompts for authorization on first launch.

**A new install starts on the persistent "Alerts" style, not auto-dismissing "Banners".** macOS gives each newly authorized app the *Banners* style, which vanishes after ~5 s, often before a tap. The bundle requests *Alerts* by declaring `NSUserNotificationAlertStyle` = `alert` in the partial `crates/lucidos-app/Info.plist` the macOS bundler merges (see `docs/desktop-app.md` § Bundle `Info.plist`). macOS reads it only when it first creates the app's Notification Center entry. So it sets the **starting** style and never overrides a choice made in System Settings → Notifications → Lucidos, across updates too.

**Tap routing: same router as the web-push tap, delivered DURABLY.** Taps arrive through the delegate's `userNotificationCenter:didReceiveNotificationResponse:` callback, on the main thread. The delegate keys off the notification `identifier`, set at `show` time to `<workspace>|<notification_id>` (see "A banner belongs to the workspace that raised it" below). That identifier looks up the show-time deep link in an in-process map. The delegate **stashes** the link, then hands the raising workspace to `route_native_tap` (`app_window.rs`), which brings up the window that tap belongs in. The OS also activates the app.

A dismiss (`UNNotificationDismissActionIdentifier`) drops the link without routing. The page (`setupNativePushTapRouting`, wired once in `startClient`) runs the link through `parseDeepLinkFromSwMessage` → `dispatchDeepLink`, the **exact** router the web-push service-worker tap uses. So a native tap marks-read and navigates identically: the `modal` default opens the notification detail, a navigate tap deep-links the thread or app.

**Nothing else may raise a window over the one the tap chose.** macOS answers an activation with a *reopen*, `applicationShouldHandleReopen:hasVisibleWindows:` → `RunEvent::Reopen`. It raises one for a Dock-icon click, a Finder re-open, and a banner tap's activation.
- The client's reopen arm acts only when no app window is visible (`reopen_shows_a_window`). Otherwise `show_main_window` would put `main` in front of the window the tap chose.
- A trayed client still comes back on a Dock click, with every window it parked rather than `main` alone (ADR 0141).
- The event's own `has_visible_windows` is unused: it counts every `NSWindow` the process owns, and the menu-bar status item owns one.

**The stash is written BEFORE any window is touched**, and the order is load-bearing. Showing or focusing a window fires that page's `focus` / `visibilitychange` drains (two of the triggers below). A stash written afterwards would land after a drain that already found nothing.

**Delivery is durable, not fire-and-forget.** A bare `app.emit` is lost when the page isn't listening: a WKWebView suspended while trayed, a reload mid-tap, or a listener still registering. So the delegate **stashes** the deep link in a pending-taps queue *and* fires `native-notification-tapped` as a wake signal.
- The page **drains** the queue (the `take_pending_native_taps` command) on FOUR triggers: its startup **cold** path, each live **warm** signal, and the window regaining **focus** / **visibility**.
- The drain is atomic in Rust (one `Mutex`-guarded partition), so a tap routes **exactly once** across all four and never re-fires on a later reload.
- The queue is soft-capped (FIFO drop), so a long-resident client that misses every signal can't grow it unbounded.
- The **warm** signal goes to one window (`emit_to`), and only when the target is an already-loaded page. A page about to load drains at startup, and an `emit` into a webview mid-navigation is dropped anyway.

**A targeted emit only lands on one window because the page-side `listen` asks for that.** Tauri's dispatch is `*listener_target == EventTarget::Any || filter(target)` (`match_any_or_filter`). A listener registered as `Any` matches **unconditionally** and hears every other window's `emit_to`.
- `utils/tauri.ts`'s `listen` registers `AnyLabel` with the window label the runtime injects (`__TAURI_INTERNALS__.metadata.currentWindow.label`).
- Under `Any`, each window's `native-window-active` transition overwrote every OTHER window's cache. A backgrounded window then ponged `is_active: true`, and its workspace stopped showing banners.
- `AnyLabel` costs nothing on the broadcast path. A plain `app.emit` dispatches with no filter, which that expression passes, so the app-update and tailscale-serve progress streams are unaffected.

**The `focus` / `visibilitychange` triggers are the load-bearing ones for a *running* client.** A banner shows only while the page is **inactive**, so a tap **always** lands on a backgrounded, often JS-throttled WKWebView.
- `route_native_tap` **shows that window without reloading it**, so the startup cold drain never re-runs.
- The warm `emit_to` is a fire-and-forget `eval` that WebKit can drop onto a just-resumed webview.
- The `focus` (window becomes key) and `visibilitychange`→visible events WebKit dispatches **in** the webview on show are eval-independent. They reliably catch the stash the warm signal missed.
- `setupNativePushTapRouting` registers them before the warm listener, so a `listen` failure can't strand them. Its returned unlisten removes all triggers. All are Tauri-gated, so browser / PWA and `tauri dev` are unaffected.

**The landed view repaints itself.** A parked WKWebView also parks the *compositor*: the layer freezes on a stale texture while the DOM updates behind it (`utils/webkitRepaint.ts`). The client recovers that on the wake, through `onPageResume`. But a deep link lands later than the wake: the drain is an IPC round trip, `openAppById` may await the apps list, and the app panel is a lazy chunk.
- So `dispatchDeepLink` calls `repaintLandedContent()` after routing. It fires the same subscribers on the thread-open burst's schedule, so a lazily-chunked view is covered.
- Same elements, no-op off WebKit. The wake-tap swallow stays unarmed, because a landed deep link is not a wake and arming it would eat the user's next tap.
- The Canvas pane's own resume repaint skips only while something is **fullscreen** over it. Its hazard is a one-frame transform capturing a `position: fixed` descendant, and only fullscreen raises that.

**Relaunch fallback → modal.** The show-time deep-link map is in-process only, so it is empty after a relaunch. The UN request `identifier` carries both halves of the notification's identity (see "A banner belongs to the workspace that raised it" below). So the delegate synthesizes a minimal `{ workspace, notification_id }` link, enough for a `Tap::Modal` default tap to open the detail *in the right workspace*. A navigate-kind tap degrades to the modal, since its tap content died with the old process. The bell badge (`NotificationCreated`) remains the durable signal regardless.

**A banner belongs to the workspace that raised it.** One packaged client process fronts the gateway, and any window can be pointed at any workspace (`/<slug>/`, ADR 0014). The pending-tap stash is **process-global**, so "the page that drains a tap" and "the workspace that raised the banner" are unrelated.

Without an identity on the tap, a banner lands in whatever workspace is open. The picker runs no drain (`setupNativePushTapRouting` is wired in `startClient`, inside the app). So a tap made on the picker was drained by the *next* workspace opened. That showed "App … no longer exists" and POSTed the mark-read to the wrong engine.

The web-push path solves the same problem with `isWorkspaceShell` in `sw.js`. The native path has the same two halves, **scope the delivery** and **open a window at the owning scope when none matches**:

- **`workspace` (the page's gateway slug) is stamped into the deep link at `show` time.** It composes the UN request identifier, `<workspace>|<notification_id>` (`notification_identifier` / `split_identifier` in `notifications.rs`). Living in the *identifier* makes the relaunch fallback attributable. It also scopes replace-by-id (the web-push `tag` equivalent), so two workspaces never collide on one notification id.
- **The drain is workspace-scoped, so delivery is not a race.** `take_pending_native_taps` takes the calling page's slug. It returns only the taps that workspace raised, plus unattributable ones (`tap_belongs_to`). Everything else stays in the stash for the window its own tap is bringing up.
- **Which window a tap belongs in is decided in Rust** (`choose_tap_target` in `notifications.rs`, applied by `route_native_tap` in `app_window.rs`). Only the client process can enumerate every window, read what workspace each is pointed at, and create one. In priority order:
  - a window already on the raising workspace is **focused**;
  - a window on **no** workspace (the picker `/~/`, or the gateway root) is **pointed at** it;
  - if the client is still booting, `desktop::launch`'s first navigation is **aimed** at it (`set_launch_target`) rather than raced;
  - otherwise a **new window** is opened there.

  A window sitting on a *different* workspace is never focused, never navigated, and never handed the tap. **A stopped owner is fine and needs no retry ladder**, unlike the `#thread=` landing channel, which has one. The gateway lazy-starts the workspace and answers with the boot splash, whose meta-refresh re-requests the same URL. The window is therefore on the right workspace by the time the real shell loads. The tap is still in the stash, waiting for its startup drain.

  A tap carrying **no** workspace falls back to the main window, dispatched by whichever page drains it. It is either a legacy direct engine (`LUCIDOS_NO_GATEWAY`, one workspace by construction), or a banner from a build older than the stamp. There is nothing to attribute it to.
- **A boot tap outranks the restored windows.** A launch reopens the workspaces
  that had a window last time (the *window session*, ADR 0123). A tap aimed at
  the boot navigation still wins `main`. The workspace it displaces becomes one
  of the extra windows instead, so nothing is lost and nothing opens twice.
- **A tap on a parked client wakes one window, not the desk.** Close to Menu Bar
  hides every window rather than destroying any (ADR 0141). A tap fronts the one
  on the raising workspace and leaves the rest parked, because a banner asks for
  one workspace. The tray's "Open Lucidos" is what brings the others back.
- **The page never redirects itself.** A page can act only on its own window. A page-side `location.assign` hop would take a window off another workspace, even one the user is mid-task in.
- **Dismiss is scoped the same way.** `dismiss_native_notification` takes the workspace. A single dismiss rebuilds the composite identifier, since a bare id matches no delivered banner. A dismiss-all enumerates delivered banners via `getDeliveredNotifications` and removes only this workspace's, never calling `removeAllDeliveredNotifications`.

### App-icon badge (Badging API + native dock tile / menu-bar tray)

The unread count is mirrored onto the installed app **icon** (surface 3 in §1), separately from the in-app bell badge. The value depends on the **origin**, because that is what decides how many workspaces one installed icon covers:

- **A gateway origin covers them all.** `gateway_manifest_json` re-stamps every manifest the gateway serves with `scope: "/"`, both the picker's (`/~/manifest.json`) and every workspace's (`/<slug>/manifest.json`), so the installed PWA can navigate between the picker and every workspace without falling out of scope and into a browser. One icon, every workspace, whichever URL it was installed from: its badge is the **aggregate**.
- **A direct engine origin covers one.** A legacy no-gateway engine, or a dev page on the engine's own port, keeps the bundled relative `scope: "."` and serves exactly one workspace. Its badge is **that workspace's own count**, and it has no control plane to ask for anything wider.

| Context | Badge value | How it's set |
|---|---|---|
| **Gateway install, on a workspace page** (`/<slug>/`) | this workspace's live count + every other workspace's | `syncWorkspaceAppBadge()` writes `crossWorkspaceUnreadTotal` (live, while open, see "re-assert, never diff" below) + the SW `push` handler reading `app_badge` (closed, Chrome/Android) + iOS reading `app_badge` natively (closed, iOS) |
| **Gateway install, on the picker** (`/~/`) | aggregate total across running workspaces | the picker (`WorkspacePicker.tsx`) sums `WorkspaceStatus.unread_count` each 2s poll and calls `applyAppBadge(total)` |
| **Direct-engine PWA** (`/` on a legacy engine or an engine port) | this workspace's unread count | the same three writers as the first row, with nothing added: `syncWorkspaceAppBadge()` skips the sum and `app_badge` carries the workspace's own count |
| **Tauri desktop app** | aggregate total across running workspaces | the desktop process (`desktop.rs`) reads the gateway's fresh `GET /~/api/v1/control/unread-total` aggregate and calls `crate::activation::apply_unread_indicator`, which writes every macOS surface that exists right now: the **menu-bar tray-icon title** ALWAYS (`notifications::set_tray_title` on the `lucidos-tray` icon), whatever the activation policy, so the menu bar is a constant read on how much is waiting; plus the **dock-tile badge** while a client window is open (Regular, `notifications::set_dock_badge`, objc2 `NSApplication.dockTile().setBadgeLabel`). Menu-bar-only (Accessory, i.e. all windows closed) has no dock tile, so the tray is then the only surface. Both on the main thread. Event-driven AND polled: the loop recomputes the instant it's **nudged** (the active workspace's webview calls the `nudge_dock_badge` command from `handleNotificationSSE` when a notification SSE arrives, whether read in-app or from another device) so the count updates without waiting; the periodic `DOCK_BADGE_POLL_INTERVAL` tick is the safety net for BACKGROUND-workspace changes whose SSE this webview never sees |

`applyAppBadge` (`store/actions/app-badge.ts`) is feature-gated (`'setAppBadge' in navigator`) and best-effort: unsupported browsers and the Tauri WKWebView no-op. The workspace-page path goes through `syncWorkspaceAppBadge()` in the same module, which is guarded off the picker context (`IS_PICKER`) and off Tauri, so each context sets exactly one value.

**The two halves of a gateway install's number come from different places, on purpose.** `crossWorkspaceUnreadTotal` (`store/actions/app-badge.ts`) adds the live `unreadCount` signal to `otherWorkspacesUnread`, a sum derived from `peerWorkspaces`. Those rows are the gateway's control listing (`GET /~/api/v1/control/workspaces`), the same one the in-app workspace switcher renders. Our own row is dropped rather than used. It reports the pre-read count for a second or two after an optimistic mark-read. Reading it back would make the icon disagree with the bell about the workspace on screen.

`refreshOtherWorkspacesUnread()` re-reads the rows at startup, on resume, on a slow visible-only interval, and when the Lucidos menu opens. A notification in another workspace already repaints the icon through its own push, so the interval is a backstop. It is best-effort: a gateway blip keeps the last-good rows rather than flashing a wrong total. It no-ops on a direct-engine origin and in the picker. It does NOT no-op under Tauri. That client badges its dock from Rust, but its page still draws the two in-app surfaces below.

**The same total is mirrored in-app, from the same computed.** Two surfaces read `crossWorkspaceUnreadTotal`, so neither can show a number the icon does not. The *Lucidos mark* carries it as a count badge (`UnreadBrandBadge`). That is the only unread count on the thread pane and the threads drawer. The bell lives in the content pane's header and never reaches either.

**The mark's badge takes the glyph's BOTTOM-right corner**, not the top-right the bell and the filter button use. The artwork's sparkle is top-right, and a resident badge there leaves the brand as three plain squares. The engine-state badge (busy, ready, pending) keeps the top corner, so the two coexist.

**The Lucidos menu then says where the number lives.** A group of rows at its top (`NotificationsMenuRows.tsx`) carries one workspace per row, this one included. This workspace's row is the live `unreadCount`, and routes in-app. A peer's row is its polled count. The whole group renders nothing, separator included, when everything is read.

**A peer row opens like any other workspace row.** `utils/workspaceWindow.ts` decides the window or the tab, so the picker, the switcher and this group cannot disagree. A plain click, a cmd-click, a middle click and the right-click alternate all behave the same on all three. What the row adds is a **landing**: the view inside that workspace to arrive on. `notifications` is the only one, and it travels as the bare `/<slug>/#notifications` hash. The target page consumes it in `hash-deeplink-router.ts`.

**Under the packaged client the landing crosses to Rust by NAME**, never as a fragment. `window_target::WorkspaceLanding` composes the URL, for the reason ADR 0028 gives. A window already on that workspace is navigated rather than merely fronted. Only a navigation carries the hash into a page that is already loaded.

**The push payload's `app_badge` follows the same origin rule, per subscription.** The engine resolves it in `app_badge_for` (`scheduler/push.rs`) from the subscription's recorded `scope_url`: a path deeper than `/` means the gateway served that page, so the payload carries the cross-workspace total the engine reads once per fan-out from `GET /~/api/v1/control/unread-total`; a root scope (direct engine) or a legacy row with no `scope_url` carries the workspace's own count. The hop is bounded and best-effort, and falls back to the own count, so a push is never delayed or dropped for a badge refinement. This is also why `sw.js` needs no cross-workspace logic of its own: it mirrors `app_badge` verbatim, and iOS reads the same field natively.

**Re-assert, never diff (the bell and the icon must never disagree about THIS workspace).** The icon badge is an **externally written surface**, written while the page is backgrounded or closed. iOS writes it from the push payload's `app_badge` in its parent process, without running the page. The SW `push` handler writes it on Chrome/Android.

So the page must *re-assert* the count at every point where it (re)establishes the unread truth. Re-asserting is an unconditional write of `unreadCount` plus the other workspaces behind the gateway. `unreadCount` is the single source the bell badge and the Unread tab project from. The points are:

- `loadUnreadNotifications`, after applying a fresh set;
- the mark-read paths (`removeFromUnread`, `markAllRead`);
- page resume (`onResume` in `store/startup.ts`), before the reload, so the icon matches the bell even when the engine is unreachable.

The `unreadCount` effect in `store/effects.ts` is *additionally* there for instant change-driven updates and the module-init clear. It cannot be the only writer: a computed whose recomputed value is equal notifies no subscriber. So a resume-time reload landing the same count re-runs nothing.

That gap left an iOS PWA showing **icon 1 / bell 0**. It followed a notification read on another device, or a tap that dropped a row this device never held. The count went 0 → 0 while the icon kept what the push wrote. A "skip the write when our count didn't change" optimization reintroduces it.

**Known boundary: a CLOSED PWA's icon can go stale.** Reads emit no push, and iOS only updates a web app's badge from a visible push. So a read on another device, while this PWA is closed, leaves its home-screen icon at the last pushed count. It corrects on the next push (which carries a fresh `app_badge`), or when the user opens or resumes the app (which re-asserts as above). Nothing on the engine side can close that window without sending a spurious visible notification.

**The aggregate is running workspaces only.** The gateway holds no DB handle (ADR 0014 §1), so it gets each count by HTTP-polling running engines. It reuses the count-only `GET /api/v1/notifications?limit=0` probe in its supervise loop. A stopped workspace reports no count and contributes 0.

A gateway install DOES navigate into a workspace, which is what the `scope: "/"` re-stamp is for. Its badge stays the total by construction rather than by confinement: the page adds the others to its own count, and a push carries the total for a gateway-scoped subscription. The Tauri count comes from a Rust loop independent of the loaded page. It likewise stays the total even when the webview is inside one workspace.

That loop reads the **fresh** on-demand aggregate, `/~/api/v1/control/unread-total`, a live count fan-out over running engines. It does not read the supervise loop's cached `last_unread`, because at nudge time (right after a read) the cached value still shows the pre-read count. The nudge and the periodic tick both use the fresh endpoint, which also avoids a flicker where a stale tick overwrites a freshly nudged value.

**Tray title and dock badge** (macOS): the menu-bar tray-icon title carries the unread count at all times, so it never depends on a window being open. Closing all client windows drops the app to the menu-bar tray. That is the Accessory activation policy, out of the Dock and Cmd+Tab (see `crates/lucidos-app/src/activation.rs` `set_menu_bar_only` / `apply_unread_indicator`). There is no dock tile there, so the tray title is the only surface showing it. Reopening a window (Regular) brings the dock badge back alongside the unchanged tray title. `apply_unread_indicator` re-applies the last known count (`LAST_UNREAD`) on each transition, so the dock tile that just appeared or vanished agrees with the tray.

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
    "tag": "<notification_id or lucidos-notification>",
    "data": {
      "notification_id": "...",
      "thread_id": "...",
      "event_id": "...",
      "app_id": "...",
      "tap": { "kind": "modal" | "navigate", ... },
      "navigate": "#notification=...  (HASH form, same params, for the Chrome SW)"
    }
  },
  "app_badge": 3
}
```

Built by `build_push_payload` in `crates/lucidos-engine/src/scheduler/push.rs`. The top-level `web_push: 8030` is the Push API spec's opt-in for declarative parsing (homage to RFC 8030). Safari 18.5+ recognizes the declarative fields. On iOS the SW `push` handler still fires and must still call `showNotification()` for a visible banner (see Net behaviour). On tap, the only reliable field before page JS runs is `notification.navigate`. `notificationclick` is not a reliable channel for installed iOS PWAs.

The top-level **`app_badge`** is the Declarative Web Push app-icon-badge field (sibling of `web_push` / `notification`). It is set at send time to the count that subscription's install should show: `0` clears the badge, and it is omitted only when no count can be read.
- It is the **only** way a *closed* iOS PWA updates its home-screen badge: iOS reads `app_badge` in its parent process WITHOUT running the service worker.
- On Chrome/Android the SW `push` handler reads it and calls `navigator.setAppBadge()`.
- The count is decided per subscription from its recorded `scope_url`. A gateway-served install gets the cross-workspace total, a direct-engine one this engine's own count (see "App-icon badge" above).

**The pushed body is size-capped; the notification row is not.** The web-push transport refuses a payload over **3052 bytes** of serialized JSON. `web-push` 0.11 checks `content.len() > 3052` on the plaintext handed to `set_payload`, *before* encryption. Its error text says "maximum payload size of 3070 characters exceeded", the wrong number and unit, so budgeting against 3070 still overflows. An unguarded long body dies inside `build()` and reaches **zero** devices, while the row and the bell badge still count it.

`fit_payload_body` in `crates/lucidos-engine/src/scheduler/push.rs` guarantees the fit before the transport sees it.
- It renders the envelope with an **empty** body and subtracts that length from the ceiling, less a small margin for JSON escaping. It truncates the body to what remains, re-measures, and shrinks again if escaping grew it.
- Nothing is hardcoded, because the overhead varies. The absolute iOS `navigate` URL comes from **this subscription's** `scope_url`, so two devices on one notification get different budgets. The Layer-3 wake adds a `wake: true` sibling, which makes it strictly tighter.
- The cut lands on a UTF-8 char boundary (real bodies carry emoji), snaps back to nearby whitespace when there is some, and appends `…`.
- Truncation logs `[Push] Truncated <kind> body to fit the 3052 B payload ceiling: …` with both lengths. The surviving `build()` failure arm says plainly that NO push reached that subscription.

**Only the body may shrink.** `web_push`, `notification.title`, both navigate URL forms, `tag`, the whole `data` block and the top-level `app_badge` survive intact. Cutting any of them breaks the tap path. A body that fits passes through byte-identical. If the envelope alone exceeds the ceiling (an enormous title, never truncated), the body is dropped and the failure logged per subscription, with no panic. The **notification row always keeps the full text**: the push is a banner, not the content of record.

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

**Stored subscription scope is load-bearing for iOS.** The page records the concrete service-worker scope URL (`scope_url`) when it subscribes or refreshes a push subscription.
- The API keeps it only when it matches the request's workspace prefix and, when the browser sends `Origin`, the same origin. It ignores malformed or cross-workspace values.
- The engine builds the absolute iOS `notification.navigate` URL from it, keeping `/<slug>/` behind the gateway.
- That avoids both known bad forms. `/?notification=…` escapes the workspace prefix to the origin-root picker. `?notification=…` is correct by URL resolution but iOS/WebKit did not apply it consistently.
- Legacy rows without `scope_url` use the query-only fallback until the next page load refreshes them.

**The browser subscription is reconciled against the engine's current VAPID key on every subscribe and refresh.** VAPID keys are per workspace (`vapid_keys` in that workspace's `preferences`, minted on first use by `get_or_create_vapid_keys`). Behind the gateway every workspace shares one origin and takes a `/<slug>/` scope. A workspace **recreated at the same slug** mints a fresh keypair while the browser keeps the old subscription at that scope. Unreconciled, the two push paths fail in opposite ways:
- `pushManager.subscribe()` rejects with `InvalidStateError` ("A subscription with a different applicationServerKey (or gcm_sender_id) already exists"). That surfaced as a dead-end *"Failed to enable push notifications"* toast.
- The silent page-load refresh re-POSTs the stale subscription. The engine then holds an endpoint whose push service rejects its VAPID signature, and notifications never arrive.

`ensurePushSubscription` (`crates/lucidos-app/src/store/actions/push.ts`) is the single chokepoint both paths go through.
- It byte-compares `subscription.options.applicationServerKey` against the engine's key. A match is returned untouched, a mismatch is unsubscribed and resubscribed.
- A browser that hides `applicationServerKey` reports "can't tell", not "mismatch". There `subscribe()` stays the authority, and one bounded retry after an `InvalidStateError` performs the same repair.
- The retry runs only when there is a subscription to drop, so an `InvalidStateError` raised for another reason cannot destroy a healthy subscription.
- Enabling push waits on `navigator.serviceWorker.ready` for the registration owning the **active** worker, as `recoverServiceWorker` does. `subscribe()` also rejects with `InvalidStateError` against a worker still `installing`.
- The follow-up `POST /api/v1/push/subscribe` clears the dead row, since `PushSubscriptionStore::subscribe` deletes by `device_id` before inserting.
- It never unregisters the service worker as a last resort, which would throw away the shell cache.

Chrome / Firefox don't recognize `web_push: 8030` and pass the JSON straight to the SW `push` handler. The handler in `sw.js` reads `data.notification.title` / `body` / `tag` / `navigate` / `data` and calls `showNotification(...)`. It keeps the `navigate` option for when Chromium ships [#382298314 "Implement Declarative Web Push"](https://issues.chromium.org/issues/382298314). `notificationclick` then routes via the SW. Same payload, two handlers: Safari natively, Chrome via SW.

Safari honors `navigate` deterministically only inside the declarative envelope, not as a `showNotification` option. So this one wire format serves the iOS-PWA path AND the future Chrome path.

### Layer 2: Liveness probe + recovery (full-wedge coverage)

Every push calls `showNotification` with `requireInteraction: true`, so a dropped click leaves the notification on screen for a retry.

The page periodically pings the SW controller with `{ type: 'lucidos:ping' }`, and the SW replies `{ type: 'lucidos:pong' }`. The recovery is `recoverServiceWorker()`: `unregister`, re-register, re-subscribe push. It runs only after **two silent windows in a row** of `SW_PROBE_TIMEOUT_MS` (5 s) each, with the page awake through both. The rules live in `crates/lucidos-app/src/utils/swLivenessProbe.ts`. `SW_RECOVERY_COOLDOWN_MS` (60 s) gates the recovery, so flapping cannot churn the subscription endpoint. On success the page shows the toast *"Notifications repaired: the service worker was unresponsive"*.

**A miss is evidence only when the page stayed awake and the next ping also failed.** An iOS PWA once toasted the repair on resume with a healthy worker:

- **A probe the page was hidden for proves nothing.** iOS freezes a hidden page. On wake, the overdue timeout can run before the queued pong, so the probe reads a live worker as silent.
- **One silent window proves nothing either.** A worker cold-starting after a wake can miss the first ping and answer the next. A wedged worker misses every window, so a second probe still catches it.

**Probe trigger points** (`store/startup.ts`, `checkSwHealth`):
- 5 s after page mount (cold-start).
- On `visibilitychange → visible`, `window focus` and `pageshow`: the "user returns to Lucidos" path.
- Every `SW_PROBE_INTERVAL_MS` (5 min) while `document.visibilityState === 'visible'`. The interval starts and stops on visibility, so hidden tabs don't burn CPU wakes.

**Probe blindness on partial wedge.** The probe uses `message`, so it reads "healthy" when only `notificationclick` is broken. This layer cannot catch the partial wedge; Layer 3 exists for that.

### Layer 3: Engine-scheduled wake-push (partial-wedge coverage, proactive)

Google's [web.dev push-notifications-common-issues guide](https://web.dev/push-notifications-common-issues-and-reporting-bugs) gives the canonical workaround for a wedged SW: send it a push. Every push event resurrects the worker, and queued `notificationclick` events drain as a side effect. Lucidos does this from the engine on a fixed delay, whether or not the user returns to the tab.

In `crates/lucidos-engine/src/scheduler/push.rs::send_push_to_all_with_app`, after fanning out the real push, the engine:
- reads each subscription's owning `devices.user_agent` and keeps macOS-Chromium devices (`is_mac_chromium`);
- dedupes per `device_id`, since a multi-tab device has several subscriptions sharing one SW;
- for each device, `tokio::spawn`s a task that sleeps `MAC_CHROMIUM_WAKE_DELAY` (3 seconds), then calls `send_wake_push_to_device(device_id, notification_id)`.

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

**UA gate is engine-side.** Device registration captures `devices.user_agent` (`POST /api/v1/devices/register`, called from `registerCurrentDevice` on every page load). See `api/settings.rs::register_device` and `core::DeviceStore::register`.
- `PushSubscriptionStore::get_push_enabled` returns it alongside the subscription tuple, so the engine asks the page nothing per push.
- Non-Mac-Chromium devices (Safari, iOS, Firefox, Tauri, Chrome on Windows / Linux) skip the wake entirely.
- Devices with `user_agent = NULL` (legacy rows from before UA capture, or a registration race) are conservatively skipped; see `pick_mac_chromium_wake_targets`.

**Silent-push budget.** Chrome counts a push as "silent" only when `showNotification` is NOT called. The wake always calls it with the original title and body, so Chrome counts it as visible.

**Failure modes.** The spawned task lives for `~3 s + a web-push round-trip`.
- The captured `Arc<LucidosEngine>` keeps the engine struct alive. But dropping the tokio runtime on shutdown aborts the task at its next `await`.
- So surviving a restart depends on `graceful_shutdown(10 s)` outlasting the 3 s sleep. A wake fired in a restart's final ~3 s is aborted, and the next real push drains the queue.
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

### History

The rules above are current. Code and docs cite these iterations by ordinal, so each keeps one line of what changed and why.

- **First**: four mitigations: `navigate` on `showNotification` (the spec bypass), a liveness probe, a page-side focus-fallback dispatcher, and a page-side wake-push (`POST /api/v1/push/wake`). The two page-side layers are gone; the other two still ship.
- **Second**: added the engine-side scheduled wake (Layer 3).
- **Third**: deleted the two page-side layers. That broke iOS tap navigation, because the engine wake is macOS-Chrome only.
- **Fourth**: moved the payload to Declarative Web Push (`web_push: 8030`), so Safari handles iOS taps natively.
- **Fifth**: moved routing into `hash-deeplink-router.ts` and wired it to every resume signal. Coverage: `hash-deeplink-router.test.ts`.
- **Sixth**: made the engine the single toast/push decision. `PresenceCheck` became a pure pong trigger, and `NotificationToastRequested` drives the toast, so a slow pong cannot cause "toast AND push".
- **Seventh**: split the navigate URL by consumer. iOS gets a cross-document query URL, since it ignores a hash-only navigation to an open window. Coverage: `declarative_navigate_is_cross_document_query_url_for_ios` and `declarative_notification_data_carries_hash_navigate_url_for_chrome_sw` (engine `push_tests.rs`).
- **Eighth**: gated the PresenceCheck on the live SSE-connection count as well as heartbeats. A long-foregrounded iOS PWA had stopped heartbeating and got pushes on top of itself.
- **Ninth**: made warm-tab delivery `postMessage` on macOS Chrome. A fragment-only `client.navigate()` resolves but routes nothing. Coverage: the deep-link routing suite in `crates/lucidos-app/src/sw.test.ts`.
- **Tenth**: made the iOS tap reload cheaper. It added Cache-first `/assets/*` (with `activate` pruning stale cache generations) and the next-macrotask cold-start dispatch. The reload itself stays.
- **Eleventh**: served the shell cache-first too, superseded by the thirteenth. It also chose not to cache the thread-list GET (see Net behaviour).
- **Twelfth**: added the native desktop OS surface (`NativePushRequested`), and let the decision run with zero web-push subscriptions. Its `mac-notification-sys` transport was replaced by the fourteenth.
- **Thirteenth**: reversed the eleventh after installed iOS PWAs went all-black. A pinned shell referenced deleted `/assets/*` bundles, and the SPA fallback answered them with `index.html`.
  - The shell is now network-first, and `/assets/*` refuses HTML (`isHtmlResponse`).
  - A black PWA self-heals on its next relaunch, once the new SW installs. A force-quit and reopen makes it immediate.
  - Coverage: the "navigation shell (network-first)" suite in `crates/lucidos-app/src/sw.test.ts`.
- **Fourteenth**: moved native banners to `UNUserNotificationCenter` via `objc2`, since `NSUserNotification` stopped delivering on macOS 26. The obj-c delivery path has no automated coverage and needs a packaged build.
- **Fifteenth**: scoped warm-tab selection to the SW's own workspace, refined by the twenty-third.
- **Sixteenth**: made iOS `navigate` scope-relative (`?…`), because `/?…` escaped to the gateway picker. Superseded by the eighteenth.
- **Seventeenth**: traced the intermittent iOS failure to a documented WebKit bug ([Progressier](https://intercom.help/progressier/en/articles/9213767-why-ios-push-notifications-sometimes-don-t-redirect-to-the-correct-url-in-a-pwa), [Apple Developer Forums 733604](https://developer.apple.com/forums/thread/733604), [firebase-js-sdk #7698](https://github.com/firebase/firebase-js-sdk/issues/7698)).
  - A tap fails to navigate iff the PWA is running AND was last launched from its home-screen icon. A cold launch, or a PWA last opened by a notification, works.
  - Skipping `showNotification` was tested and showed no notification at all, so do NOT retry it.
  - Unresolved upstream, and navigate-on-receipt would hijack the session. What is left: an Apple Feedback filing, or a non-hijacking "pending notification" affordance on resume.
  - The `[Client/sw] push` and `[Client/deeplink] *` breadcrumbs stay. Plan: `docs/plans/2026-06-19-ios-pure-declarative-push-tap.md`.
- **Eighteenth**: built iOS `navigate` as an absolute URL from the stored `scope_url`, since real devices ignored the relative query. Coverage: `declarative_navigate_uses_absolute_scope_url_for_ios` and `declarative_navigate_normalizes_scope_url_before_query_append` (engine `push_tests.rs`).
- **Nineteenth**: restored packaged-desktop IPC. Tauri 2.11 checks every IPC request from a non-local URL as `Origin::Remote`, and the window loads the gateway (ADR 0028). Every command, banners included, was rejected.
  - The app declares an ACL manifest (`crates/lucidos-app/permissions/app-ipc.json`). `desktop::launch` registers a capability for the gateway origin, pinned to the resolved port, before it navigates.
  - Capabilities are scoped by `webviews`, never `windows`, which would leak a remote grant to the `url-preview-*` webviews. The three `__panel_*_report` commands get their own any-origin capability, limited to those webviews.
  - `invoke` reports IPC failures to the engine log (`[Client/ipc]` lines via `postClientLog`). The heartbeat watchdog backs off exponentially when a reload brings back no heartbeat.
  - Coverage: the `acl_tests` suite in `crates/lucidos-app/src/lib.rs`, plus `ipcHealth.test.ts` and the watchdog-backoff tests. Open: a packaged build must confirm WKWebView sends exactly `http://localhost:<port>` as `Origin`.
- **Twentieth**: made the iOS tap reload stop announcing itself (Net behaviour, mitigation 3), keeping the network-first shell and the readiness gate. Coverage: the `notification tap (boot-splash-quiet)` suite in `crates/lucidos-app/src/utils/bootSplash.test.ts`.
- **Twenty-first**: reconciled push subscriptions against the engine's VAPID key (Layer 1). Coverage: the `stale applicationServerKey reconciliation` suite in `crates/lucidos-app/src/store/actions/push.test.ts`.
- **Twenty-second**: kept the pairing screen out of the shell cache (Net behaviour, mitigation 1). Coverage: the pairing cases in the "navigation shell (network-first)" suite, and the unpaired-install case in the shell-precache suite.
- **Twenty-third**: replaced `clientInScope` with `isWorkspaceShell`'s exact path match, so a popped-out app tab is never chosen. The `/sw.js` and skill-UI exclusions became unnecessary. Coverage: the three popped-out-tab cases in the "notificationclick handler" suite.
- **Twenty-fourth**: moved the quiet-cover decision into `<head>` (Net behaviour, mitigation 3), so a tap no longer flashes one frame of the blue launch splash. Coverage: "decides in <head>, before the splash markup can paint" in the `notification tap (boot-splash-quiet)` suite.

## §5: Test plan

Tests are named after the section ID of the rule they verify. A failing `s2_scenario_4_…` points directly at the row 4 entry in §2.

### §5.1: Unit tests

**Engine (`crates/lucidos-engine/`):**
- `s2_step_a_no_candidates_means_push_allowed`
- `s2_step_a_active_pong_means_push_not_allowed`
- `s2_step_a_only_inactive_pongs_means_push_allowed`
- `s2_step_a_pong_timeout_means_push_allowed`
- `s3_skip_presence_check_when_nobody_connected_and_no_candidate`: the gate is `max(sse_connections, candidate_count) == 0`
- `s3_run_presence_check_when_sse_connected_even_with_no_candidate`: a connected SSE page runs the check even with zero `device_presence` candidates (iOS stale heartbeat)
- `s3_expected_pong_count_is_max_of_both_signals`
- `connect_increments_and_drop_decrements` / `clones_share_the_same_count` (`api::sse_connections`, the live SSE-connection counter)
- `s3_all_candidates_pong_short_circuits_deadline`
- `s3_late_pong_after_deadline_is_dropped_with_200`
- `s3_pong_with_unknown_notification_id_returns_404`
- `s4_5_ua_predicate_matches_chrome_on_macos`
- `s4_5_ua_predicate_matches_edge_on_macos`
- `s4_5_ua_predicate_excludes_safari_on_macos`
- `s4_5_ua_predicate_excludes_chrome_on_windows`
- `s4_5_ua_predicate_excludes_chrome_on_ios`
- `s4_5_ua_predicate_excludes_firefox_on_macos`
- `s4_5_ua_predicate_excludes_empty_ua`
- `s4_5_pick_wake_targets_empty_input`
- `s4_5_pick_wake_targets_only_mac_chromium`
- `s4_5_pick_wake_targets_skips_no_device_id`
- `s4_5_pick_wake_targets_skips_no_ua`
- `s4_5_pick_wake_targets_dedupes_multi_tab`
- `s4_5_crate_ceiling_constant_matches_what_build_actually_enforces`: pins `MAX_PUSH_PAYLOAD_BYTES` against the real `web_push` builder from both sides (3052 accepted, 3053 rejected), so a crate bump that moves the threshold fails here instead of silently dropping pushes
- `s4_5_overflowing_body_still_builds_a_deliverable_message`: a 2862-char body, where the unguarded envelope reproduces `PayloadTooLarge` and the fitted one builds
- `s4_5_truncated_body_is_marked_and_shorter_than_the_original`: the kept text is the original's prefix plus the `…` marker
- `s4_5_short_body_passes_through_byte_identical`: the common case is untouched by the guard
- `s4_5_truncation_is_utf8_safe_with_multibyte_content`: emoji at every offset around the cut point stays valid UTF-8 and under the ceiling
- `s4_5_truncation_preserves_every_structural_envelope_field`: `web_push`, title, both navigate forms, `tag`, `data.*` and `app_badge` all survive the cut
- `s4_5_wake_payload_of_an_overflowing_body_also_builds` / `s4_5_wake_budget_is_tighter_than_the_original_send`: the Layer-3 wake budgets for its own `wake: true` flag
- `s4_5_envelope_alone_over_the_ceiling_degrades_instead_of_panicking`: an untruncatable title drops the body rather than underflowing the budget
- `s4_5_body_fits_exactly_at_the_ceiling_is_not_truncated`: the boundary payload is deliverable, so it is passed through
- `s4_5_wake_skipped_when_notification_already_read`: a notification read during the wake delay suppresses the wake; an unread one still fires
- `native_push_requested_serializes_with_type_tag`: the native-desktop SSE frame wire shape (`type: "NativePushRequested"` + content fields), mirroring the toast event
- `native_push_requested_is_transient_on_notification_aggregate`: never persisted; lives on the `notification` aggregate, the complement of `NotificationToastRequested`

**Frontend (`crates/lucidos-app/src/store/`):**

The native-desktop OS surface is exercised in `actions/native-push.test.ts`,
driven by the `NativePushRequested` SSE. It covers the banner on Tauri when the
frame is fresh and the page is not active, with the deep link forwarded in
SW-message shape. It covers the three no-op gates: not Tauri, a stale frame, an
active page. It covers the empty-title `Lucidos` fallback.

It also covers `setupNativePushTapRouting` **draining** the durable pending-tap
stash (`take_pending_native_taps`) through `dispatchDeepLink`. All four triggers
are exercised: the startup cold path, the `native-notification-tapped` warm
signal, window `focus`, and page `visibilitychange`. So are a multi-tap drain,
an empty-drain no-op and an off-Tauri no-op.

Last comes the **workspace scoping** the page owes. It drains with its own
`WORKSPACE_ID`, re-read per drain and `null` on a no-gateway engine. It
dispatches everything handed back, and navigates nothing.

The **window targeting** behind that is Rust-side, and unit-tested with
`cargo test -p lucidos-app`. `choose_tap_target` (in `notifications.rs`) is
covered on all five of its outcomes:

- focusing a window already on the raising workspace, preferring `main`;
- pointing a picker or root window at it;
- aiming the boot navigation while the client is still starting;
- opening a NEW window rather than taking one off another workspace;
- falling back to the main window for an unattributed tap.

Beside it, `tap_belongs_to` is covered leaving another workspace's tap in the
stash. So is `window_context` reading the slug out of a window URL, which lives
in `window_target.rs` with the URL helpers both window choosers share.

The active-device **seed** is exercised in `utils/nativeWindow.test.ts`.
`startNativeWindowActiveTracking` pulls `get_native_window_active` and seeds the
cache, correcting the `true` default, before registering the transition
listener. It keeps tracking transitions after, leaves the default on a failed
seed, and is a no-op off-Tauri.

The §4 row matrix is exercised in `__tests__/notification-toast-requested.test.ts`, driven by the `NotificationToastRequested` SSE. It covers Row 1 auto-read, Row 2/3 toast, Row 4 hidden no-toast, and null-event_id fall-through. It also covers the staleness gate (`TOAST_REQUEST_STALE_AFTER_MS`), overflow folding, and the `handleGlobalEvent('NotificationToastRequested')` wiring. The pong-only invariant lives in `actions/presence-pong.test.ts`: `s3_fresh_presence_check_within_grace_pongs_but_does_not_toast` pongs a fresh, active PresenceCheck but renders NO toast.

- `s4_row1_focused_event_in_viewport_no_toast_no_badge_marks_read`
- `s4_row2_focused_scrolled_away_toast_badge_no_mark_read`
- `s4_row3_active_other_thread_toast_badge_no_mark_read`
- `s4_row4_hidden_increments_badge_no_toast`
- `s4_null_event_id_with_focused_thread_falls_through_to_row2_toast_and_badge`
- `s4_null_event_id_with_other_thread_falls_through_to_row3_toast_and_badge`
- `s2_visibilitychange_while_visible_refreshes_device_presence_for_ios_pwa_resume`
- `s4_5_setup_routing_runs_handle_hash_location_on_visibilitychange_visible_for_ios_pwa_resume`
- `s4_5_setup_routing_skips_dispatch_when_document_is_hidden`
- `s4_5_setup_routing_runs_handle_hash_location_on_window_focus`
- `s4_5_setup_routing_runs_handle_hash_location_on_pageshow_bfcache_restore`
- `s4_5_setup_routing_runs_handle_hash_location_on_hashchange_warm_path`
- `s4_5_setup_routing_teardown_removes_every_registered_listener`
- `s4_5_setup_routing_teardown_clears_cold_start_timer`
- `s4_5_handle_hash_location_dispatches_notification_navigate_target`
- `s4_5_handle_hash_location_dispatches_query_url_for_ios_declarative_reload`
- `s4_5_handle_hash_location_strips_deep_link_hash_after_dispatch_idempotent`
- `s4_5_handle_hash_location_routes_bare_thread_hash_via_focus_thread_or_bootstrap`
- `s4_5_handle_hash_location_noops_on_unrecognized_hash_anchor`

**Service worker `notificationclick` routing (`crates/lucidos-app/src/sw.test.ts`, "deep-link routing" suite):** the macOS-Chrome warm-tap path (§4.5 "warm-tab routing").
- A warm controlled tab is focused and gets the deep link via `postMessage`, NOT a fragment `client.navigate()`. A modal tap posts it too.
- A cold tap (no open tab) opens a window at the engine-built hash URL.
- Every tap marks the source notification read and closes the OS notification.

**Multi-tab reconciliation (engine):**
- `s2_multi_tab_one_active_one_hidden_treats_device_as_active`
- `s2_multi_tab_both_pong_event_in_viewport_marks_read_once_idempotent`

### §5.2: API e2e (`crates/lucidos-e2e/tests/`)

- `s3_notification_with_visible_device_emits_presence_check_sse`
- `s3_notification_with_connected_sse_but_no_candidate_runs_presence_check`: an open SSE connection with ZERO `device_presence` candidates still triggers the PresenceCheck (iOS stale heartbeat)
- `s3_notification_with_no_candidates_sends_push_immediately`: still skips when there is also no open SSE connection
- `s3_presence_pong_endpoint_accepts_valid_payload`
- `s4_active_pong_emits_toast_request_and_suppresses_push`: toast/push exclusivity. An active pong yields a `NotificationToastRequested` SSE frame AND zero `push_log` rows for that device.
- `s4_push_allowed_emits_native_push_requested_sse_with_no_web_subscription`: a connected page with NO web-push subscription still triggers the decision. On the push-allowed branch it receives a `NativePushRequested` frame carrying the notification id.

### §5.3: Browser e2e (`crates/lucidos-app/e2e/notifications.spec.ts`)

Per Lucidos's Playwright project setup (`chromium`, `mobile`, `mobile-webkit`), the scenarios that depend on platform behavior tag the project they apply to.

- `s2_scenario_1_active_focused_event_in_viewport_no_push_no_toast_marked_read`
- `s2_scenario_2_active_focused_scrolled_away_toast_badge_no_push`
- `s2_scenario_3_active_other_thread_toast_badge_no_push`
- `s2_scenario_4_tab_in_background_tab_push_fires_badge_updates`
- `s2_scenario_5_tab_foreground_window_blurred_push_fires` *(chromium only: desktop hasFocus)*
- `s2_scenario_6_tab_closed_push_fires`
- `s2_scenario_7_two_devices_one_active_no_push_to_either`
- `s2_scenario_8_two_devices_both_hidden_push_to_both`
- `s2_scenario_9_ios_pwa_visible_no_push_to_either_device` *(mobile-webkit)*
- `s2_scenario_10_ios_pwa_hidden_push_fires` *(mobile-webkit)*

### §5.4: Test harness for OS push assertions

Browser e2e need to assert "OS push WAS sent" / "WAS NOT sent" without waiting for FCM/APNs delivery. Plan:
- Test-mode flag (env var or build-time feature) installs an in-memory push-transport stub. Real `web_push` calls instead append to an in-process `push_log` table keyed by `(device_id, notification_id, sent_at)` plus a `payload` column carrying the JSON bytes the real transport would have encrypted and sent. The payload column lets tests assert the Declarative Web Push envelope shape (`{web_push: 8030, notification: {…}}`) in addition to delivery.
- Expose `GET /api/v1/_test/push-log?since=<iso>` (mounted only when test mode is on, behind the same gating as `setup_test_db`).
- E2e helpers: `expectPushSent(page, notificationId, { deviceId?, timeoutMs? })` returns the recorded `PushLogEntry` (including the on-wire `payload` string) so tests can assert envelope shape, and `expectNoPushSent(page, notificationId, waitMs?)` proves suppression. Both poll via Playwright's `APIRequestContext` so the engine's self-signed localhost cert is trusted (Node's stricter `fetch` would reject).

The stub also bypasses the real APNs/FCM credentials, so e2e doesn't need either.

## §6: What this spec doesn't cover

- The notification payload (title, body, tap target): see the `NotificationCreated` event payload in `system-knowhow/thread-events.md`.
- The tap routing (`tap = { kind: 'modal' | 'navigate', to?: NavigateUi }`): the `notifications.tap` JSONB column and its handlers own it. This spec is orthogonal to where the tap lands.
- Permission grant UX: see `initPushSubscription` in `packages/lucidos-sdk/` and `system-knowhow/js-sdk.md` § `lucidos.notifications`.
- Server-side push delivery to APNs / FCM: `scheduler/push.rs`. This spec covers the decision to send, not the HTTP POST.
- **Targeted notifications** (aimed at one device, bypassing fan-out): not a feature today. `send_notification` fans out to every push subscription. Adding them later means a §7 amendment.
- **Batching of bursts**: the spec runs one PresenceCheck per notification (§3 "No batching").

## §7: Implementation notes

The design removed code paths the old one relied on:

- **Deleted `ThreadPresenceStore`, the `thread_presence` table, the `ThreadFocused` / `ThreadUnfocused` events, and the frontend `/api/v1/thread-presence` POST loop.** PresencePong carries `focused_thread_id` live, so no persisted thread-focus index is needed. A migration dropped the table.
- **`device_presence` is the candidate index only.** `record_visible` / `record_hidden` / the 30s heartbeat tell the engine whether to RUN the PresenceCheck, alongside the SSE count (§3).
- **Deleted `DevicePresenceStore::any_visible()`.** The pong-based decision replaced the global suppression query.
- **Test-mode push log endpoint** (`GET /api/v1/_test/push-log`) is gated by the Cargo feature flag `e2e-test-hooks`. The e2e workspace builds with it on; production binaries do not include the endpoint.
- **Backwards compatibility: synchronous deploy.** The web app pulls the latest bundle on page load, so an active page always runs code that can pong. A missing pong times out to push-allowed, which covers the window between an engine restart and the next page load.
