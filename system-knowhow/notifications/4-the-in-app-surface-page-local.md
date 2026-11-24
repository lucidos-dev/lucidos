---
name: Notification Routing, §4: The in-app surface (page-local) (continued)
description: Part of Notification Routing. Its index lists the sections it covers.
---

One part of `system-knowhow/notifications`, which lists every part.

## §4: The in-app surface (page-local) (continued)

- **`tap.kind === 'modal'` (the notification detail).** `viewNotification` resolves the row from whichever loaded list holds it (the browse list `notifications` or the unread set `unreadNotifications`) and opens synchronously.
  - This is sound because both lists carry WHOLE notifications. `NotificationStore::get_filtered` and `get_by_id` select an identical column list and serialize the same `Notification`.
  - Only a genuine miss fetches: in practice the cold push-tap deep link, since a warm page has loaded the unread set for its badge. The fetch reveals the pane at once with `NotificationDetailInline`'s skeleton, delay-gated past `SPINNER_DELAY_MS`.
  - The `panelOverlay` is written only with a real notification in hand, so a failed fetch leaves no phantom nav-stack entry. The pending id lives beside it in `notificationDetailPending`.
- **`tap.kind === 'navigate'` to a thread** (the common case by far: 80% of notifications in a mature workspace). `focusThreadOrBootstrapResult` focuses synchronously when the thread is in `threadMap`.
  - Otherwise it focuses **optimistically** and reveals the thread pane before fetching metadata, so `ThreadView` renders its delay-gated skeleton instead of a dead interval.
  - `awaitedThreadId` protects optimistic focus from stale-pointer cleanup.
    `releaseAwait` restores prior focus after failure. A queue-held thread stays
    awaited until its metadata arrives.
  - The metadata comes from `GET /api/v1/threads/:id`, never the grouped `GET /api/v1/threads`. The grouped call assembles saved, recent archive, active, composing and family base, costing hundreds of milliseconds on a large workspace.
  - A cold push tap always meets an empty map, because the deep link dispatches while `loadAllThreads` is in flight. So this path runs on every such tap.

**A thread-targeted tap is refused at the PRODUCER unless its id names a thread.** A notification is written and pushed in one stroke. So a tap the page cannot resolve is a dead deep link by the time anyone sees it, read as `Thread "<id>" no longer exists`. Every producer settles `tap.to.id` first, through `notifications::resolve_thread_tap_id`, and `create_notification` re-runs it as the chokepoint backstop. A missing id is refused too: the router's own "you forgot the id" would reach a banner nobody can repair.

`send_notification` resolves the `current` / `this` alias to **the thread the notification is about**, which is the `link_thread` its own `thread_id` column carries. Usually that is the calling thread. Where a trigger fired on another thread's event, it is the ORIGIN thread instead. So the row's two buttons agree, and the tap never lands in the trigger's own transcript. `navigate_ui` has no such notion and resolves to its calling thread. `POST /api/v1/notifications` and the SDK's `POST /api/v1/ui/navigate` pass no caller, so the alias is refused there with every other non-uuid: neither has a thread of its own.

Reading is deliberately unguarded. `Tap`'s decode still accepts a non-uuid id, because rows written before the guard have to stay readable to be repaired. The migration `20260918051213_repair_alias_notification_tap_thread_ids.sql` did that, from each row's own `thread_id`.

**Where a `navigate` tap LANDS inside the thread, and what happens when it can't.** The *source event* is resolved in the DOM by `[data-event-id]` (`scrollToEventAndPulse` in `components/chat/scrollState.ts`), scrolled to, and left carrying the *navigation focus marker*. Three rules govern which element that is:

- **Every deep-linkable event is addressable, whether or not it starts a turn.** An event that begins an exchange (`UserQuestionAsked`, `CodingAgentPermissionRequest`, `CommandPermissionRequested`, `McpConsentRequested`, …) stamps the whole turn, and the pulse narrows to its `.initiator-panel`. An event folded into a turn as a STEP stamps the card that renders it instead, and the pulse stays on that card. Today that is `ResponseFailed`, whose failure card is what the user is being sent to see. Landing on the whole turn would bury the failure in a long coding-agent run. The same `[data-event-id]` lookup backs Row 1's in-view check below, so its auto-mark-read now works for those events too.
- **An event that stamps nothing is reached through the turn that CONTAINS it.** The two stamps above are the whole set. `stampedEventIds` in `store/thread-events/exchange-anchors.ts` declares it. A source-scan tripwire in `deep-link-anchor.test.ts` fails if `ChatExchange` grows a third without declaring it. So every other event, such as an ordinary tool call or a `CodingAgentIdled`, is addressable only via its turn. `deepLinkAnchorForEvent` does that re-targeting: the event itself when it stamps its own element, its exchange's starter otherwise.

  Every event deep link uses it, through `landOnEvent` (`store/actions/threads.ts`). The engine makes sure a notification's event lives in the thread it opens (`verify_event_anchors`, above). It does not make sure the event stamps its own element. A form request raised mid-turn, such as a `CredentialRequested`, is a step, so its tap lands on the turn holding it. The anchor is asked again on every look, because a cold tap only knows it once the thread's events arrive.

  The *event wait* step's **show it** goes further, because a wait can match ANY event type in ANY thread. In practice it matches a `CodingAgentIdled` in whichever thread it was watching. That step resolves the owning thread first, through `GET /api/v1/events/:event_id/location`. It answers `thread_id: null` for a workspace domain event, and 404 for an event id with no row. The step then re-targets, and hands off to the same `focusThread(threadId, { targetEventId })` path a notification tap uses.
- **A fullscreen app panel is the whole viewport, so a thread tap leaves it.** `handleNavigationRequest`'s `thread` branch calls `exitAppFullscreen()` before it focuses, exactly as `new-chat` does. Only fullscreen goes, and `panelOverlay` stays. The split gives the conversation its own pane, so closing the app would cost the reader what they were working in.
- **A target with no BOX is waited for, and every link says how it ended.** A collapsed Conversation pane zeroes the target's own rect, and an ancestor clipped to nothing shows none of a target that still measures full size. `isElementVisible` rejects both, and gaining a box is a layout change no `MutationObserver` can see. So the link watches that box with a `ResizeObserver` too (`watchForABox`), inside its existing deadline. Whatever the ending, it writes one `[Client/deeplink] outcome` line to `engine.log`. That line names the ending, how many elements matched, how many had a box, and where the transcript was going to rest.
- **A target that never renders is reported, not silently dropped.** The resolve waits out a deadline (4s) for a lazily-loading thread. If the event is still missing, a warning toast says the event is not shown in this thread. No pulse, since there is no element to mark, and **no scroll**: the transcript stays where it was. The bottom of the thread is not the place the tap asked for (the *reading position* entry in `docs/glossary.md` has the wider rule). The Changes-panel deep link (`data-change-id`) shares the same deadline and reports the same way.

**The notification's `title` is the toast's title, and a producer must never restate it in the `body`.** `presentInAppNotification` passes the two straight through: `showToast(body, 'info', { title })`. The title is the bold line. The body is plain text under it: a newline is a line break, and a `"• "` line is shown as written. A notification with no body shows its title alone, not bold.

So write the body as *content only*: `"• "` lines for a list, a bare sentence for a single item. The toast, the detail pane's `<h2>` and the OS push banner each show the title already, so a body that repeats it shows it twice.

**The body may be markdown, and only the detail pane renders it.** The toast, the macOS banner and the web push show text as written. So the engine sends them a plain-text copy (`plain_text_body` in `scheduler/notification_plain_text.rs`). Markup goes, a list item starts with `"• "`, and a link keeps its label. The notification row keeps the markdown. A body that forwards a question card therefore reads cleanly on every surface.

**A body that names a Settings page links it as `settings:<view>`**, with the same view id as `navigate_ui settings_view`: `[Settings → System → Backup](settings:backup)`. Add `#<row>` to land on a row of the page, as `settings:providers#xai` does, matching `navigate_ui`'s `fragment`. The label is the breadcrumb route, so every plain-text surface still reads the route. The engine's backup and disk-space notifications build theirs from one `SettingsPage` value (`scheduler/notifications.rs`). That value also builds the tap, so the link and the tap open the same page.

**The passive `tap: "none"` kind is RETIRED: every notification is openable.** It was a button-less variant that auto-marked-read on show and auto-dismissed after 5 s. It went so a notification never renders as a dead banner (see `docs/plans/2026-07-02-remove-notification-tap-none.md`). Readers can still meet old `none` data, handled like this:
- Nothing produces `none`. The `send_notification` tool schema and the SDK `Tap` type don't offer it, and the DB `notifications_tap_valid` CHECK rejects it for new writes.
- A migration rewrote existing `{kind:none}` rows to `{kind:modal}`.
- Historical `NotificationCreated` events keep `{kind:none}` forever (event-sourcing immutability). The engine's custom `Tap` deserialize coerces them to `Modal` on replay, so a projection rebuild never re-emits a `none` row.
- The frontend `resolveDeepLink` demotes any stray `none` (an old SW message or stale URL) to the openable `view-notification` path. It is never a passive `mark-read` or a no-op.
- Row 1 (auto-read because the user is looking at the source event) applies to every kind.

The toast's triggers are the `PresenceCheck` SSE, and `NativePushRequested` on a desktop window that came forward since it answered. **Why not fire it on `NotificationCreated`?** iOS PWAs queue SSE messages while backgrounded. After an OS push tap, the queued `NotificationCreated` flushes once the user has landed, so `isPageActive()` is `true`. The matrix would then pick Row 2/3 and add a duplicate toast. The PresenceCheck's freshness gate (§3) drops a check past its deadline, so a late flush never pops a toast on a resume.

**On `PresenceCheck` received via SSE (and not dropped by the §3 freshness gate), the page acts, then pongs:**

```ts
const outcome = presentInAppNotification({ title, body, target }); // the matrix below
submitPong(notification_id, { device_id, outcome });
// submitPong goes through the TRANSPORT: direct connection POSTs at once, a
// shared one hands the answer to the holder to merge with its peers.
```

**The shell must be what the user is looking at, not merely a visible page.** `shellIsActive()` (`store/actions/in-app-notification-toast.ts`) is `isPageActive()` AND no app fullscreen, covering both the native mode (`appFullscreenHost`) and the iOS CSS mode (`appPseudoFullscreen`). The render and the pong read this one predicate, so they cannot disagree.

An app filling the screen therefore takes the **OS push**, not the toast. Two reasons:

- **Only the shell can show a toast**, so presence answers "can we reach this person without pushing?". A reader deep in a fullscreen app is not reading the shell.
- **It makes fullscreen agree with a popped-out app window.** The two are identical from where the user sits, and a popped-out one has always taken the push.

An app merely open in the content pane is unaffected: the shell around it renders the toast. The *device-presence heartbeat* is deliberately unaffected too. It reports whether the page is visible, which under a fullscreen app it still is, and the engine reads it only to count expected pongs.

**`presentInAppNotification` applies the §4 row matrix, and returns what it did:**

```ts
if (workspaceUnavailable()) return 'not_shown';
if (!shellIsActive()) return 'not_shown';          // Rows 4 / 4b: bell badge only, the OS push is the surface.
const onSource = target.event
  && focusedThreadId.value === target.thread
  && isInViewport(target.event);
if (onSource) {
  markReadOptimistic(target.notification);         // Row 1: looking at the source event.
  return 'auto_read';
}
if (!currentNotificationToasts()) return 'silenced';
showToast(...);                                    // Row 2 / 3: active, scrolled away or other thread.
return 'shown';
```

**The Rows 2/3 toast is switchable: `notification_toasts`, a global preference, default `true`.** Set it to `false` and the toast never renders, the `+N more` overflow toast included. The notification still counts on the bell badge and waits in the Notifications panel, the deferred queue the switch sends it to. It stays unread there, even when it points at the place the reader is at (see "A notification that lands where you already are" below). Settings → Appearance & Behavior → Notifications carries the row as **In-app toasts**.

Unlike `push_notifications` the switch is workspace-wide, so one write covers every device. The page reads it on every toast and reloads it on `PreferencesChanged`, so a change applies with no reload.

**No OS push arrives in its place.** The page reports `silenced`, which suppresses the push like `shown` does (§3), so a present device stays out of the fan-out. The user asked not to be interrupted, not to be interrupted differently. The gate sits in `presentInAppNotification` **below** the Row 1 branch, so reading the source event still auto-marks it read. It is deliberately not in `showToast`, which also serves toasts answering the user's own action (an apply-change result, an error).

**Outcomes across devices:**
- All devices hidden → push fans out to all subscriptions. No in-app toast anywhere.
- One device active, others hidden → push suppressed globally; in-app toast on the active device only.
- Multiple devices active → push suppressed globally; in-app toast on every active device whose §4 row resolves to toast.
- Active device pongs as Row 1 (event in viewport) → no push to anyone and no toast on this device (auto-read). Any OTHER active device on a different thread gets a toast.

**Reconnection.** Page load hydrates the unread set from `GET /api/v1/notifications?filter=unread`, so a notification that arrived offline shows in the badge. Toasts missed while offline are NOT replayed: the unread set is the durable signal.

**Read marks are global.** Reading a notification on one device clears the badge on all others via the `NotificationRead` broadcast. Auto-read in Row 1 emits the same event as a manual read.

**Cross-device OS-banner dismiss (macOS desktop only).** A read also removes the *already-delivered OS banner* on other devices, but only on the native macOS desktop client.
- When a read flips (`NotificationRead`, or `NotificationsAllRead` for mark-all), the engine broadcasts a transient `NativePushDismissRequested`. It carries `notification_id` (`Some(id)` for one, `None` for all) and `sent_at_ms`.
- A connected Tauri desktop app removes the matching delivered banners via `UNUserNotificationCenter.removeDeliveredNotifications(withIdentifiers:)`. It drops the stashed deep link so a phantom tap can't route (see the native section below).
- A `None` removes every banner **the reading workspace raised**, never another workspace's.
- The page handler `handleNativePushDismiss` gates on `isTauri()` (browser / PWA ignore the event) and on the `sent_at_ms` freshness budget. That budget bounds, but does not close, the window where a late dismiss-all clears a banner created after an all-read.
- There is **no** active-window gate, because removal is a harmless no-op when nothing matches.

This is the **one platform where cross-device dismiss is both possible and deterministic**: the desktop app stays SSE-connected, and native notifications have no "must show something" rule. The **open web cannot** do it. Safari revokes a Web Push subscription after 3 silent pushes, and Chrome/Firefox show a default "site updated in background" banner on a silent push. So browser / PWA banners persist until swiped, while the in-app badge still syncs via `NotificationRead`.

A **native iOS app** could dismiss via the same UN API. That is deferred and best-effort, since a backgrounded iOS app's SSE is suspended and would need a silent APNs push: see `docs/plans/2026-06-19-ios-native-apns-app.md`. The abandoned web-only approach (a visible "✓ Read on another device" tombstone) is in `docs/plans/2026-05-18-cross-device-notification-dismiss-design.md`.

