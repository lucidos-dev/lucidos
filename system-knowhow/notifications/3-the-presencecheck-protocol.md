---
name: Notification Routing, §3: The PresenceCheck protocol
description: Part of Notification Routing. Its index lists the sections it covers.
---

One part of `system-knowhow/notifications`, which lists every part.

## §3: The PresenceCheck protocol

**Trigger.** When `NotificationCreated` is emitted, the engine decides whether to run the PresenceCheck from two signals (`expected_pong_count` in `crates/lucidos-engine/src/scheduler/push.rs`):

1. **Live SSE-connection count** (`engine.sse_connections.count()`, the number of open `GET /api/v1/events` streams). This is the **primary, authoritative** gate: a connected page can pong however stale its heartbeat is. It increments when a page opens its EventSource and decrements when the stream drops, so it can't go stale.
2. **`device_presence` candidates**: rows fresher than `PRESENCE_STALE_AFTER` (120s, `crates/lucidos-core/src/core/device_presence.rs`). A 30s heartbeat (`device-presence.ts:HEARTBEAT_INTERVAL_MS`) refreshes them, plus a forced refresh on `visibilitychange-while-visible`. This secondary signal covers a page that heartbeated recently but whose SSE connection just dropped (network blip).

A `DeviceHidden` deletes the device's row and stamps `devices.last_seen_at` with the hide time. So the agent's Known devices list still reads a hidden device as seen at the hide, not at its last page load. The stamp plays no part in this gate. `navigate_ui` reads the same row to tell the agent whether its target device had Lucidos visible.

The engine runs the PresenceCheck when `max(sse_connections, candidate_count) > 0`, waiting for that many pongs. The deadline short-circuits at the first suppressing pong, or once they all arrive. It sets `push_allowed = true` directly **only when both are zero**: nobody is reachable (the "phone in your pocket" case).

**A connection owes exactly one pong, and that is a client-side contract.** The count is connections, not documents, so everything sharing a connection answers once. The *shared SSE holder* does that by merging its documents' answers (§ Multi-tab above). Two consequences show up in traces:

- An app frame has **no presence voice**. The shell around it relays its frames off the shell's own connection, and the frame never answers. An app in its own tab attaches to the holder and never answers either. So a workspace with only app tabs open, and no shell anywhere, pongs nothing and takes the OS push.
- A connection whose documents all decline to answer produces no pong. The engine waits out `DEADLINE_MS` and pushes, which is correct: there was no shell to show a toast.

**Why the SSE gate, not heartbeat freshness alone.** iOS suspends the 30s heartbeat timer while a PWA is foregrounded, and never fires `visibilitychange`. So an active page's `device_presence` row ages past 120s while its EventSource stays open and would pong `shown`. A heartbeat-only gate skipped the PresenceCheck and pushed on top of the active page. The SSE count doesn't depend on the heartbeat, so the active page always gets to pong.

**Ping.** The engine broadcasts a transient SSE event to every connected page. It **carries the toast**, so a page can act on it before it answers:

```json
{
  "type": "PresenceCheck",
  "data": {
    "notification_id": "<uuid>",
    "title": "<string>",
    "body": "<string>",
    "thread_id": "<uuid or omitted>",
    "event_id": "<uuid or omitted>",
    "app_id": "<string or omitted>",
    "tap": { "kind": "modal" },
    "deadline_ms": 2000,
    "sent_at_ms": 1700000000000
  }
}
```

The deadline (`DEADLINE_MS` in `crates/lucidos-engine/src/scheduler/push.rs`) is sized for an iOS PWA reaching the engine over cellular or a Tailscale tunnel. Real traces show two RTT bands for the SSE-out plus pong-POST trip:
- Steady-state cellular or Tailscale relay: 400–800 ms.
- The first packet after the phone radio resumes from idle pays the Tailscale (userspace WireGuard) path renegotiation: 1100–1800 ms.

The deadline is how long the engine waits for a page's report. A report that lands after it means the page showed the toast AND the push went out. That is loud and logged, never silent (see "Failure handling"). `notify_one` wakes the engine at the first suppressing pong, or once every expected device has answered (see `run_presence_check`). So a larger deadline costs latency only when a `device_presence` row is stale (page killed without firing `DeviceHidden`).

`sent_at_ms` is the engine's wall clock at emit, and drives the freshness gate below.

**Freshness gate.** iOS PWAs suspend JS in the background and queue SSE messages until the page is visible again. When the user taps the OS push, the queued PresenceCheck flushes long after the deadline. The page compares `Date.now() - sent_at_ms` against `deadline_ms` (`handlePresenceCheck` in `presence-pong.ts`). Past it, the page shows nothing and pongs nothing: the engine has already pushed, and a toast now would only repeat it. It reads `deadline_ms` off the payload, so an engine bump flows through automatically.

**Act, then pong.** Every receiving page runs the §4 row matrix on the check (`presentInAppNotification` in `in-app-notification-toast.ts`) in one synchronous step. It puts the toast on screen, marks the row read, or holds it for the badge. Only then does it POST `/api/v1/presence-pong`:

```json
{
  "notification_id": "<uuid>",
  "device_id": "<string>",
  "outcome": "shown"
}
```

| `outcome` | What the page already did | Suppresses the push |
|---|---|---|
| `shown` | The toast is on screen, alone or folded into the overflow toast (Rows 2 and 3). | yes |
| `auto_read` | The source event is on screen, so it marked the row read (Row 1). | yes |
| `silenced` | In-app toasts are off, so the row waits on the bell badge. | yes |
| `not_shown` | The shell is not in front of the reader (Rows 4 and 4b). | no |

- A disconnected or otherwise unavailable workspace returns `not_shown` before checking shell activity.
- The shell is in front when `shellIsActive()` holds: `isPageActive()` (visibilityState + hasFocus on desktop, visibilityState only on iOS PWA) and no app fullscreen.
- Row 1 needs the source event in view. The page locates it in the DOM (`document.querySelector('[data-event-id="..."]')`). If absent (virtualized list, not rendered) it is not in view; if present → an `IntersectionObserver` / `getBoundingClientRect()` check.
  - The rect is intersected against the **transcript scroll container's** band, not the window's. The app header and the prompt region inset the transcript. An event behind either strip is inside `window.innerHeight` yet hidden, and calling it visible would suppress a needed notification.

**Decision.** At the first suppressing pong, or after `deadline_ms`, the engine decides.
- `push_allowed = !any(pong.outcome suppresses)`. Pongs are grouped by `device_id` and merged within a device (see §2 "Multi-tab"); across devices it is the same test.
- If every candidate device ponged before the deadline, the engine decides immediately.
- **`push_allowed = false` → the engine sends NO push.** A page already has the notification in front of the reader.
- **`push_allowed = true` → the engine fans out the OS push**, and emits `NativePushRequested` for a desktop app (§4).
- The push follows what a page DID, never what it promised. So a page that hides after answering keeps its toast, and nothing leaves the reader with neither surface.

**No batching across notifications.** A burst of 10 notifications in 5 seconds produces 10 independent PresenceChecks. Each spawns its own deadline timer and they run in parallel. So burst latency is one deadline window (~2 s in the worst-case "stale candidate" branch), not 10 × `DEADLINE_MS`. Batching is a future optimization; the spec does not depend on it.

**Failure handling.**
- A device that got the ping but failed to POST a pong → treated as `not_shown`. The page is likely dying or the network unreliable, and pushing is the safer side.
- A pong arriving AFTER the deadline, or for an unknown `notification_id` → discarded with a `200 OK` ack. It is never a `404` or `409`: this race is normal. A late *suppressing* pong is logged (`[PresenceTracker] … arrived after the push decision`): that page showed the toast and the push went out too.

**Why this works without silent-push risk.** The engine decides server-side BEFORE sending the push. If it sends, the service worker always calls `showNotification()`, so no browser sees a push the SW swallows. The `userVisibleOnly: true` subscription contract stays clean, and the browser never penalises the origin for "ghost" pushes.

**Cost.** Up to one `DEADLINE_MS` (currently 2 s) of added latency when a reachable device fails to pong. Two cases produce a non-ponging but counted device:
- A `device_presence` row still fresh because the page was killed without firing `DeviceHidden`.
- An SSE connection open at the socket level whose page can't run JS (a heavily throttled or just-suspended background tab).

When every expected device pongs, `notify_one` wakes the engine at the last pong, so there is no latency tax. An *active* page settles the decision with its own pong, so the common "toast, no push" path does not wait for anyone else. With nobody reachable (the phone-in-your-pocket case), there is no delay at all.

## §4: The in-app surface (page-local)

The bell badge and toast are **two distinct triggers** that decode the same notification:

1. **Bell badge**: driven by SSE `NotificationCreated`. The handler, `handleNotificationSSE` in `notifications.ts`, reloads the **unread set** (`unreadNotifications` in `store.ts`). It reads `GET /api/v1/notifications?filter=unread`, capped at the API's 100. It fires on every connected page, active or hidden, so the badge stays in sync.
   - **The badge count is DERIVED, not fetched.** `unreadCount` is a `computed` over `unreadNotifications.length`, so no separate count can drift from the notifications (bell showing unread over an empty inbox).
   - **The reload is ordered.** Every load takes the next epoch as it starts, and an unread load applies only if no newer one has started.
   - **A read is a pending intent until the server acknowledges it** (ADR 0437). `unreadNotifications` is the served set (`servedUnreadNotifications`) minus the pending reads. A load started before the read POST's acknowledgement still masks the row, however late it lands. Only a load started after it shows server truth again, and a refused POST shows the row again.
   - That covers **in-app auto-read** (Row 1): a page fires a created-reload, then marks the row read. The reload lands with the row and the row stays masked, so the notification can't flash on the badge. The same holds for a cold-start load in flight under a push tap, and for Mark all read.
   - Page reload hydrates the set fresh from the same endpoint, and there is no cached count.
2. **Toast**: driven by SSE `PresenceCheck` (`handlePresenceCheck` in `presence-pong.ts`, then `presentInAppNotification` in `in-app-notification-toast.ts`). The check carries `title` / `body` / `thread_id` / `event_id` / `app_id` / `tap` / `sent_at_ms`. Active pages render the toast (or auto-read on Row 1) and report it; hidden pages show nothing and report `not_shown` (Row 4). The engine pushes only when no page reported handling it (§3 "Decision").

   The toast once waited for a separate frame sent after the engine's decision. A page that hid in between dropped it, so the reader got neither surface (History, twenty-fifth). Acting on the check itself closes that window; a pong that misses the deadline now gives toast AND push, which is loud.

**A producer that names a source event gets a `navigate` tap without asking for one.** `default_tap` (`scheduler/notifications.rs`) is what an omitted `tap` resolves to. Both producer entry points use it: the `send_notification` LLM tool and `POST /api/v1/notifications`. It answers `Navigate` to the notification's own `thread_id` and `event_id` when BOTH are present, `Modal` otherwise. An explicit `tap` always wins, including an explicit `{"kind":"modal"}`, and nothing is backfilled.

**Both entry points refuse an event the linked thread does not hold** (`verify_event_anchors`, beside `default_tap`). The page looks for an anchor only inside the thread it opens, so any other event makes a dead tap. The check covers two pairs, each on its own: the row's `thread_id` with its `event_id`, and a thread tap's `to.id` with its `to.event_id`. It refuses an id that names no event, an event in another thread, and a workspace domain event, which lives in no thread. The tool returns an error the agent can act on, and the HTTP route returns a 400. (A trigger fired by a domain event once passed that event's id, and the reader met "That event is not shown in this thread".)

Why `event_id` and not `thread_id`? An `event_id` is the producer naming a specific thing to look at: a question, a permission request, a failure card. The destination is the point, and a card in between is a wasted tap. A `thread_id` ALONE is deliberately not enough. The engine stamps one on every `send_notification` from the origin thread, so it is provenance rather than intent. Navigating on it would drop a daily-summary notification into the middle of that trigger's own agent transcript.

**A tap anywhere on the toast opens it, and the X defers it. There is no button, no auto-open and no `[OK]`.** For `tap.kind === 'modal'` (the default, or a missing `tap`) and `tap.kind === 'navigate'`, the Rows 2/3 toast opens on a tap anywhere on the card (its `onClick`). The tap runs `dispatchDeepLink`. For `modal` it opens the notification detail in the content pane (the `view-notification` action); for `navigate` it goes to the destination. It also marks the notification read.

The toast **persists** (no auto-dismiss). The close (X) button dismisses **without** marking read, so the row stays unread in the bell badge and panel. There is deliberately **no `[OK]` / acknowledge button**: marking read *without* opening would bury an unanswered question. The detail panel **never auto-opens** when the toast appears (`presence-check-toast.test.ts` pins this). No toast moves keyboard focus, so an unsolicited one cannot steal focus mid-typing and pre-arm a reflexive Enter.

`view-notification` opens the detail in the content pane (`panelOverlay = { type: 'notification-detail' }`, rendered by `NotificationDetailInline`), not a modal overlay. `tap.kind === 'modal'` is the wire name of the tap, not a UI modal.

**A toast lives exactly as long as its row is unread.** The bell badge, the Unread tab and the toast are three projections of one set (`unreadNotifications`), so a read leaves a toast nothing backs. Two inputs enforce it, and they answer different questions.

**The unread-set watch answers a read this page can see.** `installNotificationToastLifetime` (`in-app-notification-toast.ts`, wired from `store/effects.ts`) subscribes the toasts to that set and removes the toast of every row that leaves it. It is the fast half, answering on the tick the reader acts, because that drop is optimistic and local. It covers the **seen target** rule below, the Notifications panel, the detail chevrons, and Mark all read. Three properties of it are deliberate:

- **It fires on the TRANSITION, never on bare absence**: the ids the set held a moment ago, minus the ones it holds now. A toast rides `PresenceCheck` while the reload `NotificationCreated` kicked off is still in flight, so a brand-new row reads as absent too. Dropping on absence would hide a notification nobody has seen.
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

