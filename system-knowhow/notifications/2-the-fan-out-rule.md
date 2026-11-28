---
name: Notification Routing, §2: The fan-out rule
description: Part of Notification Routing. Its index lists the sections it covers.
---

One part of `system-knowhow/notifications`, which lists every part.

## §2: The fan-out rule

**Step 0: the audience (team workspaces only).** Every notification records who it is for, as device ids on `NotificationCreated` and its row (ADR 0623). A producer may name them. Otherwise, a notification that opens a thread is for its *root thread*'s audience. The engine finds the thread from the tap's thread target, else from `event_id`. The audience is the devices of the root thread's *driver* and *watchers*.

- It is the whole team, as everywhere else, when the gateway knows fewer than two people. It is the whole team too when the root thread has no driver, has an escalated gate, or names nobody who holds a device. So a gate always reaches someone, and a personal workspace pushes exactly as before.
- Web push skips subscriptions outside the audience. `PresenceCheck` and `NativePushRequested` carry `audience` (device ids), and a page outside it shows nothing and pongs `not_shown`.
- The inbox still lists every notification, but one outside your audience reads as read: it never lights your bell badge or your app badge.
- A lookup that fails reaches the whole team.

**Read state is per user on a team gateway.** With two or more users, a read from a device a user holds is that user's: it clears the notification on their other devices and leaves everyone else's unread. Mark all read works the same way, and so does clearing the in-app toast and the desktop banner: `NativePushDismissRequested` carries the reader's devices as `audience`. A caller no user holds reads and writes the team's shared flag, and so does the agent's `notifications` tool. With one user, everything reads the shared flag, as before. Each device's push badge counts what its own user has not read.

**User-only messages (team workspaces only, ADR 0649).** A *user-only message* raises one notification for its mentioned users plus the root thread's driver and watchers, never the sender's own user. A message that wakes the agent and mentions users tells only the mentioned. Either way a mentioned user with no role becomes a watcher first.

A user-only message's notification records a *push group* on `NotificationCreated` and its row. Every push in the group carries the tag `user-<thread_id>`, so a burst replaces one banner, and an **Unwatch** action (see Layer 1). iOS draws no action buttons, so there it is a plain banner.

**Thread users notices (team workspaces only).** Three notifications reach one or a few users besides the gates:

| Notice | Sent when | For |
|---|---|---|
| "Kim answered your gate" | someone answers a gate on a root thread another user drives, and so takes it over | the user who drove it |
| "Kim took over" | someone taps **Take** on a root thread another user drives | the user who drove it |
| "Thread done" | the root thread is archived | its driver and watchers, except whoever archived it |

Each opens the root thread. Nobody hears one on a gateway with fewer than two users, and nobody whose user holds no device.

**Config change notices (team workspaces only).** A *config change* posts a notification naming who made it: a repository, trigger, MCP server or credential added, changed or removed (`engine/config_change.rs`). It reaches everyone but the user who made it. A change by an agent, or by a device no user holds, reaches the whole team. A change nobody made posts nothing: an engine write, an identity collapse, a one-shot trigger deleting itself.

Where the event recorded enough to invert it, the tap is `{"kind": "undo-config-change", "event_id": "<uuid>"}`. Tapping it, or **Undo** in the detail, asks to confirm and then calls `POST /api/v1/config-changes/:event_id/undo`. The engine writes the inverse through the same store, as the tapping device, so an undo is a config change of its own. It answers 409 `moved_on` when a later change touched the same thing, or the thing is already gone or already back. So an undo never overwrites someone's later edit. It answers 409 `not_undoable` for:

- a credential value change, since a value never reaches an event;
- an MCP server re-registration or removal, since its old env is sealed and its owned credentials are deleted.

Undo restores a deleted credential's metadata with an empty value, for someone to enter again. It never overwrites a credential saved under the name since. A restored trigger keeps its slug unless another trigger took it, and then gets a fresh one. Only the engine writes this tap: a producer that sends one is refused.

**Step A: global push check (once per notification).** Did a page already put this notification in front of the reader?

The engine runs the PresenceCheck protocol (§3) to find out. Each page acts on the check first and then reports its outcome. After Step A, the engine has a boolean `push_allowed` and zero or more `PresencePong` records.

- `push_allowed = true` if and only if NO pong reported `shown`, `auto_read` or `silenced` within the deadline.
- The engine skips the protocol and sets `push_allowed = true` directly only when **nobody is reachable**. That means zero open SSE connections (`GET /api/v1/events`) AND zero `device_presence` candidate rows fresher than `PRESENCE_STALE_AFTER` (120s, `crates/lucidos-core/src/core/device_presence.rs`).
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

¹ An active page acts on the check and reports it, so `push_allowed = false` for everyone. The exception is a report that lands after the deadline: the push has gone out by then, so that page has the toast AND the push. The engine logs it (§3 "Failure handling").

² Only sent when Step A returned `push_allowed = true`. Otherwise no push to this device either.

³ The page gets the `NotificationCreated` SSE and increments the bell badge, but renders NO toast. It reports `not_shown`, so the OS push is the surface, and a toast waiting on return would only repeat it. The bell badge is the durable, on-return signal.

⁴ The page is visible, but the shell is not the surface in front of the user. It shows no toast and pongs `not_shown`, so the notification takes the OS surface (see §4 for why). A fullscreen app thus agrees with a popped-out app window, which has always been Row 5. Rows 1 to 3 all require an ACTIVE shell.

**OS-push transport: web push vs native desktop (Rows 4 & 5).** "OS push" in the matrix is a *surface*, not one transport:
- **Browser / PWA** → web push (APNs / FCM) to the device's stored `push_subscriptions` row, rendered by the service worker.
- **Tauri desktop app** → a native macOS notification. When `push_allowed = true` the engine broadcasts a `NativePushRequested` SSE. The connected desktop page renders the banner and routes its tap via `show_native_notification` (§4). Both transports fire on the same branch, so the per-device matrix is unchanged.
- A **fully quit** Tauri app is Row 5 (offline), with a caveat. The bundled desktop build ships the engine *inside* the app, so quitting the app stops the engine too. Native delivery therefore covers Row 4, and Row 5 only while a separate engine is still reachable. A quit single-process install re-syncs the bell badge on next launch.

**The OS-surface decision runs for any connected client, not just web-push subscribers.** `send_push_to_all_with_app` runs the PresenceCheck decision (and so the toast or the native banner) whenever any client is reachable. Reachable means a web-push subscription **or** an open SSE connection or fresh heartbeat. So a desktop-only workspace with zero web-push subscriptions still gets in-app toasts and native banners. The engine does nothing only when there is no subscription **and** nobody connected.

**The fan-out never holds up whoever created the notification.** `send_push_to_all_with_app` starts the fan-out on its own task and returns at once. A trigger, a backup run or the event bus goes on while pushes are in flight. That task keeps the caller's event-trigger chain depth.

**A quiet device never delays another.** The web pushes go out to every device at once. Each send gets 30 seconds (`PUSH_SEND_TIMEOUT`), because the transport sets no deadline of its own. A send that runs out of time fails for that subscription only: the engine logs it and keeps the subscription (silence is not a 410).

**Row 1 requires a non-null `event_id`.** `notifications.event_id` is nullable. Scheduler errors, ad-hoc `send_notification` calls without an event ref, and audit events have no source event. With a null `event_id`, the device falls through to Row 2 (if the thread matches) or Row 3.

**Multi-tab / multi-window per device.** Two Lucidos tabs on the same browser share `device_id` (per-origin localStorage). Where the browser has `SharedWorker` they also share **one** SSE connection, held by the *shared SSE holder*. The holder merges their answers into the single pong that connection owes. Otherwise each tab holds its own connection and POSTs its own pong. The reconciliation is the same either way:
- For Step A: the strongest outcome across the device's answers wins (`shown` > `auto_read` > `silenced` > `not_shown`). If ANY tab handled the notification, the push is suppressed.
- For Step B's auto-read (Row 1): if ANY tab reports "event in viewport on source thread", the auto-read endpoint is hit once. `POST /api/v1/notifications/:id/read` is idempotent.
- Step B for the in-app surface runs **per tab**. Each tab checks its own focused thread and viewport on SSE receipt.

**Where the merge happens is what keeps the count honest.** The engine waits for one pong per open SSE connection, so N documents behind one connection owe one answer, not N. Aggregating in the holder makes that true by construction. Per-document answers would let a background tab settle the push while a foreground one was still answering.

**Worked example.** macOS Chrome with Lucidos in a background tab; iOS PWA with the screen off in your pocket.
- Step A: Chrome's `device_presence` row was deleted when the tab went background (visibilitychange fired DeviceHidden). Its **EventSource stays open**, so it counts toward the live SSE-connection gate and the engine runs the PresenceCheck. Chrome shows no toast and pongs `not_shown` (`hasFocus()` is false). The suspended iOS PWA has its EventSource closed and never pongs. No suppressing pong arrives → `push_allowed = true`.
- Step B for Chrome: row 4 → OS push fires; the bell badge updates via SSE when the page regains focus.
- Step B for iOS: row 5 → OS push fires via APNs.
- Both devices get a push.

If this scenario shows "Chrome bell only, no OS push", the bug is one of these. Tests in §5 cover each.
- (a) The visibilitychange event didn't fire DeviceHidden, leaving Chrome's presence row as a stale candidate.
- (b) Chrome was never subscribed to push (no permission grant, no service worker registration).
- (c) The push was sent but Chrome dropped it.

