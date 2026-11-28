---
name: Notification Routing, §4: The in-app surface (page-local) (continued)
description: Part of Notification Routing. Its index lists the sections it covers.
---

One part of `system-knowhow/notifications`, which lists every part.

## §4: The in-app surface (page-local) (continued)

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

**The matrix above is untouched, and the strict measure is why.** For an event target this rule's condition IS Row 1's condition, so a notification arriving while its card is on screen was already being auto-read. Row 2 is the case a looser "the thread is open" reading would have broken: on the source thread with the event scrolled away, the card is out of band, so this rule stays silent and the toast still fires. Rows 3, 4 and 5 name places the card cannot be visible in. Row 1 keeps its own code because it does a second job this rule does not, suppressing the toast. A place target has no card, so it needs the rule below to leave the matrix alone.

**A notification that lands where you already are waits for a later visit.** Being at a place is not seeing a notification that lands there. Its surface is the Rows 2/3 toast, or the bell badge with toasts off. An agent's `send_notification` taps to its own thread, where the reader usually is, so this is the common case.

- When `NotificationCreated` reaches an active page, the visit under way does not read its place target (`noteArrivalWhilePresent`). The hold is set before the unread reload that the dwell waits on, so no dwell can beat it.
- A PresenceCheck that this page drops as stale, or answers `not_shown`, releases the hold (`noteArrivalWhileAway`). The reader was away when it arrived, as on an iOS PWA that flushes its queue on resume, and the OS push was its surface.
- Leaving the place ends that visit, and so does the page going inactive. The next visit reads it as usual.
- An event target is not held. Its card is either in view on arrival, which is Row 1, or it is not.

So the toast stays up while the reader stays put, since its row stays unread. With toasts off, the bell badge keeps the count.

**A `modal` tap is never cleared this way**, since its place is the notification detail and opening that already marks it read. Neither is a notification matched on its own `thread_id`: that column is provenance, not a destination (see "Why `event_id` and not `thread_id`?" above), so reading a trigger's own transcript must not clear the daily summary it produced there. Only `tap.to` names a target. The rule marks read and nothing else: it never scrolls, never archives, and never clears the Blocked state. What the read then clears is not its to decide, and the row's toast goes with its badge under the lifetime rule above.

**A *read request* rides the same watch, with the same dwell** (ADR 0409). It points at no event, so its target is the end of the thread's newest reply: the bottom edge of the last turn in the visible band. It counts only once the turn has ended, and only on the thread the reader has open. A thread still waiting on an event or its sub-threads counts too.

Seeing it posts `POST /api/v1/threads/:thread_id/read-request/seen` with the summary version the page holds. The engine's `ThreadReplySeen` then clears the request on every device. If the thread changed since, the post answers 409 and the next sample reports again. It has no notification row and touches none.

One set of constructors (`store/actions/visitKeys.ts`) spells both the place a tap names and the place the shell is showing, so the two cannot drift apart. Four navigate targets name no revisitable place and are excluded: `url`, `new-chat`, `new-app` and `new-trigger`. A test walks the generated `NAVIGATE_TARGETS` so a new one has to say which it is.

### Native desktop OS surface (Tauri)

The Tauri desktop app embeds a WKWebView, which has no service worker and can't subscribe to Web Push. The native banner fills that gap, driven page-side from the same SSE stream.

Ordinary packaged Cmd-W and red-button closes hide every app window, retaining
its page and existing workspace connection. Close to Menu Bar hides all app
windows through the same operation. Each hide marks only its own window
inactive for notification presence and records its watchdog hide intent.
The last visible app window disappearing leaves the client in the menu bar.
Reopen restores parked windows and skips individually closed windows (ADR 0816).
A notification tap can still reuse a closed window on its workspace.

Native hide/show intent also has its own per-window bridge:
`get_native_window_shown` seeds it and `native-window-shown` publishes transitions.
It stays separate from focus: an unfocused window can still be shown.
The listener registers before the seed so a concurrent transition takes precedence.

A newer frontend can run inside an older installed desktop shell.
Only an exact missing-command or ACL-denied response for this command marks the protocol unsupported.
Those shells keep unknown native visibility and the previous eligibility policy, while retaining shared claims, locks and receipts.
They cannot exclude hidden windows using a protocol they lack.
Other initialization failures remain actionable errors; Retry starts a fresh attempt and concurrent callers share it.

Window peers announce that state while retaining their connections and life locks.
Interactive forms, navigation, push-enable prompts and OAuth toasts prefer shown
peers before thread, authorization and recency preferences. All-hidden windows
keep existing routing. Captures and widget-fit targeting keep their existing policy;
browser and PWA selection does too.

These interactive requests use shared claims in a workspace/device IndexedDB
journal, serialized by Web Locks (ADR 0721). BroadcastChannel wakes peers after
commits; delayed announcements cannot acknowledge or discard a request.
Forms share one request identity between live SSE and pending-request sync.

Sync queues open forms newest first and checks earlier journal entries for persisted resolution events.
`GET /api/v1/form-requests/resolved` checks up to 100 comma-separated `request_ids` per call, including archived threads.
Absence from the pending list alone never proves resolution, since archiving also hides a form there.
This retires receipts for forms answered while the client was offline without resolving newly arriving requests.
Archived forms remain suspended in the journal until the server offers them again.

Other requests use the SSE envelope event identity, separate from notification tap targets.

Acceptance records presentation before waiting for a form answer or push permission.
A resolved form leaves a tombstone even when its request arrives later.
A crash between UI acceptance and receipt commit produces an uncertain delivery toast with Retry.
Retry can repeat a presentation, so use it only when the request still needs attention.

Unavailable storage, coordination capabilities or native visibility produce an actionable error.
Native visibility transitions and window state changes reconsider pending work automatically.

Development and deliberate teardown keep genuine closes. Retention does not
guarantee WebKit execution or native banner delivery (ADR 0718).

**This whole surface needs the client process to exist.** The engine records notifications with no client running, but the client shows the banner (`show_native_notification`). So the packaged build installs a *login agent* (`com.lucidos.client`, see `docs/desktop-app.md`) beside the always-on service, and a restarted Mac still gets banners. A login-started client is menu-bar-only with a hidden window (the trayed state below), so it takes the native-banner branch.

- On the **push-allowed branch** (`push_allowed = true`, §3 "Decision") the engine emits a `NativePushRequested` SSE *alongside* the web-push fan-out. It carries the same toast content as the `PresenceCheck` (`title` / `body` / `thread_id` / `event_id` / `app_id` / `tap` / `sent_at_ms`).
- The page handler (`handleNativePushRequested` in `native-push.ts`) gates, then **always surfaces the notification**:
  - **not Tauri → ignore**: browser / PWA already got the real web push on this branch;
  - **stale per `NATIVE_PUSH_STALE_AFTER_MS` → ignore**: a late SSE-queue flush, as with the PresenceCheck's freshness gate;
  - otherwise it runs `presentInAppNotification`, the same step the check runs. The page answered from behind other windows, but the reader may have brought it forward since. If the shell is in front now, they get the toast (or auto-read, or the badge with toasts off). Only `not_shown` takes the banner. Dropping the frame there would leave them neither.

  The banner invokes the app's `show_native_notification` command (`utils/tauri.ts`). It passes the deep-link target in the SW-message shape (`notification_id` / `thread_id` / `event_id` / `tap`).
  - **The trayed-window case depends on the native-active bridge (§1).** Without it, a window trayed by macOS `orderOut:` keeps reporting `visibilityState='visible'` / `hasFocus()=true`. It then renders an invisible toast and reports `shown`, so the engine suppresses the push, leaving only the bell badge. With the bridge, a trayed or unfocused client reports inactive and gets the native banner. A window behind another app already reports inactive via `hasFocus()`, but only the bridge fixes the trayed case.
  - **The bridge is SEEDED on page load, not just transition-driven.** The `native-window-active` cache (`utils/nativeWindow.ts`) defaults to `true`. Otherwise only the transition events Rust's `on_window_event` emits update it (`Focused` / `tauri::WindowEvent::CloseRequested` / `show_main_window`).
  - Tauri does **not** replay those events to a late listener. A page reloaded while backgrounded or trayed would keep the `true` default, render an invisible toast, and suppress its own push. That covers a crash-watchdog reload (`lib.rs`), the "New version → Refresh" reload, and a cold or unfocused launch.
  - `startNativeWindowActiveTracking` reads `get_native_window_active`
    (`focused && visible && !minimized`) before registering its transition listener.
    A failed read keeps the cached value, initially `true`, and still registers
    the listener. Off-Tauri it does nothing and keeps `true`.
    - **The seed reads the CALLING page's own window, not `main`.** Tauri injects the calling `tauri::Window` into the command, as `focus_calling_window` and `window_ready_to_show` do. Reading `main` let a backgrounded second window seed itself **active** off a focused first one.
- **Why a direct `objc2` `UNUserNotificationCenter` path, not a crate.** Two crates were tried and dropped.
  - `tauri-plugin-notification`'s desktop `show()` is fire-and-forget and never reports the click (its `onAction` is mobile-only), so it can't route taps.
  - `mac-notification-sys` (what `notify-rust` and the plugin use on macOS) drives Apple's **deprecated `NSUserNotification` API**. That API no longer delivers on recent macOS (silent on macOS 26 "Tahoe").
  - The shipped path drives Apple's **`UserNotifications`** framework through `objc2`. It builds a `UNMutableNotificationContent`, posts a `UNNotificationRequest` (no trigger = deliver now), and installs a `UNUserNotificationCenterDelegate` to capture taps. See `crates/lucidos-app/src/notifications.rs`, which the `show_native_notification` command forwards to.
- **The delegate implements `willPresent`, so banners show even if the app is frontmost.** macOS suppresses a frontmost app's banner unless the delegate opts in via `userNotificationCenter:willPresentNotification:`. Without it, a stale-false active signal makes `show()` fire while Lucidos is focused, and the banner silently drops. The delegate returns `[.banner, .list, .sound]`. One window never double-surfaces: it shows a banner only after its own outcome came back `not_shown`, so a frontmost window takes the toast instead. Two windows on one workspace can: if the reader brings one forward between the check and the frame, that one toasts while the other raises the banner.
- **Cross-device dismiss (removal counterpart).** The same `objc2` path also *removes* delivered banners. The `dismiss_native_notification` command (`notifications::dismiss`) calls `removeDeliveredNotifications(withIdentifiers:)`.
  - It removes one banner, or every banner the calling workspace raised (enumerated via `getDeliveredNotifications`). The engine's transient `NativePushDismissRequested` SSE drives it (see §4 "Cross-device OS-banner dismiss").
  - It also drops the banner's stashed deep link from the in-process map, so a removed banner can't route.
  - Both arms take the workspace (see "A banner belongs to the workspace that raised it" below). `handleNativePushDismiss` gates on `isTauri()` + `sent_at_ms` freshness, with no active-window gate. Inert in `tauri dev` / off macOS, like `show`.

**Enabling.** Desktop has no web-push subscription to create, and the macOS `UserNotifications` path has no JS-queryable permission. macOS shows banners per the app's authorization, requested once at startup (`notifications::setup`) and changeable in System Settings → Notifications. So "enabling push" on the Tauri app is a no-op success. That is the same `initPushSubscription` entry point `preferences(action="set", key="push_notifications", value="enabled")` and the settings toggle drive.

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
- Under `Any`, each window's `native-window-active` transition overwrote every OTHER window's cache. A backgrounded window then read itself active, and its workspace stopped showing banners.
- `AnyLabel` costs nothing on the broadcast path. A plain `app.emit` dispatches with no filter, which that expression passes, so the app-update and tailscale-serve progress streams are unaffected.

