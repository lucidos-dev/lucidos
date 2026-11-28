---
name: Notification Routing, §4.5: Service worker wedge (mitigation stack) (continued)
description: Part of Notification Routing. Its index lists the sections it covers.
---

One part of `system-knowhow/notifications`, which lists every part.

## §4.5: Service worker wedge (mitigation stack) (continued)

### History

The rules above are current. Code and docs cite these iterations by ordinal, so each keeps one line of what changed and why.

- **First**: four mitigations: `navigate` on `showNotification` (the spec bypass), a liveness probe, a page-side focus-fallback dispatcher, and a page-side wake-push (`POST /api/v1/push/wake`). The two page-side layers are gone; the other two still ship.
- **Second**: added the engine-side scheduled wake (Layer 3).
- **Third**: deleted the two page-side layers. That broke iOS tap navigation, because the engine wake is macOS-Chrome only.
- **Fourth**: moved the payload to Declarative Web Push (`web_push: 8030`), so Safari handles iOS taps natively.
- **Fifth**: moved routing into `hash-deeplink-router.ts` and wired it to every resume signal. Coverage: `hash-deeplink-router.test.ts`.
- **Sixth**: made the engine the single toast/push decision. `PresenceCheck` became a pure pong trigger. The now-retired event `NotificationToastRequested` drove the toast, so a slow pong could not cause "toast AND push".
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
  - Worker push and notification-click breadcrumbs and mark-read requests carry the originating push's `notification.data.restore_generation` as `x-lucidos-restore-generation` (`null` becomes `none`). The engine captures that generation before presence waits and includes it in ordinary and wake envelopes before fitting their size. A delayed notification never borrows a page's current generation. Legacy payloads and asset-failure logs have unknown provenance; their explicit `unknown` header fails generation admission, so those worker logs do not reach the engine.
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
- **Twenty-fifth**: reversed the sixth. A page that hid after answering lost both surfaces: its answer suppressed the push, and it dropped the later toast frame. Now the `PresenceCheck` carries the toast, and the page acts before it answers with an outcome. A desktop window that came forward since answering gets the toast, not a dropped banner. Why: ADR 0440. Coverage: `__tests__/presence-check-races.test.ts`.

### User-only message recovery

Recorded user mentions retain delivery intent across engine shutdown.
Recovery freezes each recipient user's key and devices before applying watches.
Each user has a separate stable notification identity and presence decision.
Handling a toast on one user's devices cannot suppress another user's OS push.
Each web subscription has a separate completion receipt, so a failed subscription never replays completed targets.

A native desktop client acknowledges `/api/v1/notification/native-receipt` after OS acceptance or in-app handling.
Updated native clients must return explicit OS acceptance.
Older clients retain their successful-command completion behavior, including the risk of losing an OS-rejected banner.

A dropped broadcast or rejected native banner remains pending for recovery.
Unread pending delivery has no expiry; a receipt or reading the notification ends retry.
Recovery preserves the first PresenceCheck decision, so retries do not repeat in-app toasts.
Delayed Chromium wakes target only subscriptions newly delivered in that attempt, preserving receipt boundaries across scopes.

Completion receipts survive engine shutdown. External acceptance followed by a lost receipt can repeat an alert, with the same notification identity.
Stable identities replace existing entries rather than stacking them.
Thread deletion serializes with recovery before locking its family rows, preventing deleted threads from being reannounced.

## §5: Verification

Use the test files below as the coverage map. Test titles may change;
the section numbers describe routing rules rather than a test-name convention.

### Engine and API

- `crates/lucidos-engine/src/scheduler/push_tests.rs`: candidate selection,
  pong outcomes, deadlines, multi-tab suppression and platform predicates.
- `crates/lucidos-e2e/tests/api_support/notifications_presence_test.rs`:
  the HTTP notification and presence protocol.
- `crates/lucidos-engine/src/scheduler/push_test_log.rs` and
  `crates/lucidos-agents-api/src/api/notifications.rs`: the test-only push log.
  `GET /api/v1/_test/push-log` requires the `e2e-test-hooks` Cargo feature.
  Production binaries omit this endpoint.

### Frontend

Paths below are relative to `crates/lucidos-app/src/`.

| Test file | Coverage |
|---|---|
| `store/actions/in-app-notification-toast.test.ts` | The §4 row classifier and shell activity. |
| `store/__tests__/presence-check-toast.test.ts` | PresenceCheck dispatch, toast outcomes, auto-read and overflow. |
| `store/actions/presence-pong.test.ts` | Act before answering; ignore expired checks. |
| `store/__tests__/presence-check-races.test.ts` | Visibility changes around a pong or native banner. |
| `store/actions/push-device-toggle.test.ts` | Device opt-in, permission refusal and silencing other phones. |
| `store/actions/notification-deeplink.test.ts` | Warm and cold notification routing. |
| `store/actions/native-push.test.ts` | Native banner routing and freshness. |
| `sw.test.ts` | Service-worker notification clicks and deep links. |

### Browser

`crates/lucidos-app/e2e/notifications-all.spec.ts` verifies closed detail panels,
row navigation, infinite scrolling, event deep links, auto-read, toast preferences
and the Declarative Web Push payload.

The suite uses `expectPushSent` from `e2e/helpers.ts` to inspect the test push log.
It checks transport decisions without requiring live APNs or FCM credentials.

## §6: What this spec doesn't cover

- The notification payload (title, body, tap target): see the `NotificationCreated` event payload in `system-knowhow/thread-events.md`.
- The tap routing (`tap = { kind: 'modal' | 'navigate', to?: NavigateUi }`): the `notifications.tap` JSONB column and its handlers own it. This spec is orthogonal to where the tap lands.
- Permission grant UX: see `initPushSubscription` in `packages/lucidos-sdk/` and `system-knowhow/js-sdk/lucidos-preferences-user-settings.md` § `lucidos.notifications`.
- Server-side push delivery to APNs / FCM: `scheduler/push.rs`. This spec covers the decision to send, not the HTTP POST.
- **Targeted notifications** (aimed at one device, bypassing fan-out): not a feature today. `send_notification` fans out to every push subscription. Adding them later means a §7 amendment.
- **Batching of bursts**: the spec runs one PresenceCheck per notification (§3 "No batching").

## §7: Implementation notes

The design removed code paths the old one relied on:

- **Deleted `ThreadPresenceStore`, the `thread_presence` table, the `ThreadFocused` / `ThreadUnfocused` events, and the frontend `/api/v1/thread-presence` POST loop.** The page decides Row 1 itself when the check arrives, so no persisted thread-focus index is needed. A migration dropped the table.
- **`device_presence` is the candidate index only.** `record_visible` / `record_hidden` / the 30s heartbeat tell the engine whether to RUN the PresenceCheck, alongside the SSE count (§3).
- **A device hand-over moves the presence row with the device.** `DevicePresenceStore::move_device` repoints the old id's row to the new id. If the new id is a registered device with its own row, its row wins and the old one is dropped. A row under an unregistered new id belongs to no device, so it is cleared and the old row moves in. Every push subscription moves to the new id too, so a re-paired page keeps its candidate row and its push.
- **Deleted `DevicePresenceStore::any_visible()`.** The pong-based decision replaced the global suppression query.
- **Test-mode push log endpoint** (`GET /api/v1/_test/push-log`) is gated by the Cargo feature flag `e2e-test-hooks`. The e2e workspace builds with it on; production binaries do not include the endpoint.
- **Older pages fall back to push.** An open page can retain an earlier bundle.
  A missing pong, or an old pong without the required `outcome`, times out to
  push-allowed. The next page load fetches the current bundle.

## Diagnostic evidence

Service-worker breadcrumbs retain a separate diagnostic code.
Only recognized codes are shareable; notification text and breadcrumb data remain private.

Follow [Agent Sandbox](../agent-sandbox.md) for bounded diagnostic access and the raw-host-log restriction.
