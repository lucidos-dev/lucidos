---
name: Notification Routing, §4: The in-app surface (page-local) (continued)
description: Part of Notification Routing. Its index lists the sections it covers.
---

One part of `system-knowhow/notifications`, which lists every part.

## §4: The in-app surface (page-local) (continued)
### Native desktop OS surface (Tauri) (continued)

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
  - a window already on the raising workspace is **focused**. A shown or minimized one beats a window closed into hiding. Among those, the window focused last wins, then `main`, then the lowest label;
  - a window on **no** workspace (the picker `/~/`, or the gateway root) is **pointed at** it;
  - if the client is still booting, `desktop::launch`'s first navigation is **aimed** at it (`set_launch_target`) rather than raced;
  - otherwise a **new window** is opened there.

  A window sitting on a *different* workspace is never focused, never navigated, and never handed the tap. **A stopped owner is fine and needs no retry ladder**, unlike the `#thread=` landing channel, which has one. The gateway lazy-starts the workspace and answers with the boot splash, whose meta-refresh re-requests the same URL. The window is therefore on the right workspace by the time the real shell loads. The tap is still in the stash, waiting for its startup drain.

  A tap carrying **no** workspace falls back to the main window, dispatched by whichever page drains it. It is either a legacy direct engine (`LUCIDOS_NO_GATEWAY`, one workspace by construction), or a banner from a build older than the stamp. There is nothing to attribute it to.
- **A boot tap outranks the restored windows.** A launch reopens the windows
  that were open last time, each under its recorded label (the *window
  session*, ADRs 0123 and 0585). A tap aimed at the boot navigation still wins
  `main`. The window it displaces becomes an extra window instead, under a
  fresh label. The first restored window on the tapped workspace is skipped, so
  nothing is lost and nothing opens twice.
- **A tap on a parked client wakes one window, not the desk.** Close to Menu Bar
  hides every window rather than destroying any (ADR 0141). A tap fronts the one
  on the raising workspace and leaves the rest parked, because a banner asks for
  one workspace. The tray's "Open Lucidos" is what brings the others back.
- **The page never redirects itself.** A page can act only on its own window. A page-side `location.assign` hop would take a window off another workspace, even one the user is mid-task in.
- **Dismiss is scoped the same way.** `dismiss_native_notification` takes the workspace. A single dismiss rebuilds the composite identifier, since a bare id matches no delivered banner. A dismiss-all enumerates delivered banners via `getDeliveredNotifications` and removes only this workspace's, never calling `removeAllDeliveredNotifications`.

### App-icon badge (Badging API + native dock tile / menu-bar tray)

The unread count is mirrored onto the installed app **icon** (surface 3 in §1), separately from the in-app bell badge. The value depends on the **origin**, because that is what decides how many workspaces one installed icon covers:

- **A gateway origin covers them all.** `gateway_manifest_json` re-stamps every manifest the gateway serves with `scope: "/"`, both the picker's (`/~/manifest.json`) and every workspace's (`/<slug>/manifest.json`). The installed PWA can therefore navigate between the picker and every workspace without falling out of scope and into a browser. One icon, every workspace, whichever URL it was installed from: its badge is the **aggregate**.
- **A direct engine origin covers one.** A legacy no-gateway engine, or a dev page on the engine's own port, keeps the bundled relative `scope: "."` and serves exactly one workspace. Its badge is **that workspace's own count**, and it has no control plane to ask for anything wider.

| Context | Badge value | How it's set |
|---|---|---|
| **Gateway install, on a workspace page** (`/<slug>/`) | this workspace's live count + every other workspace's | `syncWorkspaceAppBadge()` writes `crossWorkspaceUnreadTotal` (live, while open, see "re-assert, never diff" below) + the SW `push` handler reading `app_badge` (closed, Chrome/Android) + iOS reading `app_badge` natively (closed, iOS) |
| **Gateway install, on the picker** (`/~/`) | aggregate total across running workspaces | the picker (`WorkspacePicker.tsx`) sums `WorkspaceStatus.unread_count` each 2s poll and calls `applyAppBadge(total)` |
| **Direct-engine PWA** (`/` on a legacy engine or an engine port) | this workspace's unread count | the same three writers as the first row, with nothing added: `syncWorkspaceAppBadge()` skips the sum and `app_badge` carries the workspace's own count |
| **Tauri desktop app** | aggregate total across running workspaces | the desktop process (`desktop.rs`) reads the gateway's `GET /~/api/v1/control/unread-total` aggregate and calls `crate::activation::apply_unread_indicator`, which writes every macOS surface that exists right now: the **menu-bar tray-icon title** ALWAYS (`notifications::set_tray_title` on the `lucidos-tray` icon), whatever the activation policy, so the menu bar is a constant read on how much is waiting; plus the **dock-tile badge** while a client window is open (Regular, `notifications::set_dock_badge`, objc2 `NSApplication.dockTile().setBadgeLabel`). Menu-bar-only (Accessory, i.e. all windows closed) has no dock tile, so the tray is then the only surface. Both on the main thread. Event-driven AND polled: the loop recomputes the instant it's **nudged** (the active workspace's webview calls the `nudge_dock_badge` command from `handleNotificationSSE` when a notification SSE arrives, whether read in-app or from another device) so the count updates without waiting; the periodic `DOCK_BADGE_POLL_INTERVAL` tick is the safety net for BACKGROUND-workspace changes whose SSE this webview never sees |

`applyAppBadge` (`store/actions/app-badge.ts`) is feature-gated (`'setAppBadge' in navigator`) and best-effort. Unsupported browsers and the Tauri WKWebView no-op. The workspace-page path goes through `syncWorkspaceAppBadge()` in the same module. That path is guarded off the picker context (`IS_PICKER`) and off Tauri, so each context sets exactly one value.

**The two halves of a gateway install's number come from different places, on purpose.** `crossWorkspaceUnreadTotal` (`store/actions/app-badge.ts`) adds the live `unreadCount` signal to `otherWorkspacesUnread`, a sum derived from `peerWorkspaces`. Those rows are the gateway's control listing (`GET /~/api/v1/control/workspaces`), the same one the in-app workspace switcher renders. Our own row is dropped rather than used. It reports the pre-read count for a second or two after an optimistic mark-read. Reading it back would make the icon disagree with the bell about the workspace on screen.

`refreshOtherWorkspacesUnread()` re-reads the rows at startup, on resume, on a slow visible-only interval, and when the Lucidos menu opens. A notification in another workspace already repaints the icon through its own push, so the interval is a backstop. It is best-effort: a gateway blip keeps the last-good rows rather than flashing a wrong total. It no-ops on a direct-engine origin and in the picker. It does NOT no-op under Tauri. That client badges its dock from Rust, but its page still draws the two in-app surfaces below.

**The header counts the notification rows in its menu.** `notificationsMenuUnreadTotal` sums the shared `notifyRows` selector, including reachable paired remote gateways. The *Lucidos mark* uses this total for its count badge (`UnreadBrandBadge`), accessible label and tooltip. The OS icon retains its origin-scoped total, so the header can show more unreads than the icon.

The mark carries the only unread count on the thread pane and the threads drawer. The bell lives in the content pane's header and never reaches either.

**The mark's badge takes the glyph's BOTTOM-right corner**, not the top-right the bell and the filter button use. The artwork's sparkle is top-right, and a resident badge there leaves the brand as three plain squares. The engine-state badge (busy, ready, pending) keeps the top corner, so the two coexist.

**The Lucidos menu then says where the number lives.** A group of rows at its top (`NotificationsMenuRows.tsx`) carries one workspace per row, this one included. This workspace's row is the live `unreadCount`, and routes in-app. A peer's row is its polled count. The whole group renders nothing, separator included, when everything is read.

**A peer row opens like any other workspace row.** `utils/workspaceWindow.ts` decides the window or the tab, so the picker, the switcher and this group cannot disagree. A plain click, a cmd-click, a middle click and the right-click alternate all behave the same on all three. What the row adds is a **landing**: the view inside that workspace to arrive on. `notifications` is the only one, and it travels as the bare `/<slug>/#notifications` hash. The target page consumes it in `hash-deeplink-router.ts`.

**Under the packaged client the landing crosses to Rust by NAME**, never as a fragment. `window_target::WorkspaceLanding` composes the URL, for the reason ADR 0028 gives. A window already on that workspace is navigated rather than merely fronted. Only a navigation carries the hash into a page that is already loaded.

**The push payload's `app_badge` follows the same origin rule, per subscription.** The engine resolves it in `app_badge_for` (`scheduler/push.rs`) from the subscription's recorded `scope_url`:

- **A path deeper than `/`** means the gateway served that page. Normal and wake pushes carry that recipient's cross-workspace total. The engine requests `GET /~/api/v1/control/unread-total?recipient-device-id=<device>` once per distinct recipient in each fan-out. Only authenticated local processes may select a recipient. Browser callers without that parameter receive their own authenticated count; they cannot override the recipient.
- **A root scope** (direct engine) or a legacy row with no `scope_url` carries the workspace's own count.
- **A remote workspace route** (`/<slug>@<gateway>/`) as the first segment means another gateway relayed the page. That origin's icon counts workspaces this engine cannot see, so the payload omits `app_badge` and leaves the icon alone. The tap still uses that scope, so it opens the page on the relaying gateway's origin.

Recipient requests run concurrently within the existing 1500 ms deadline. A failed count falls back only to that recipient's own workspace count. Missing or unknown recipients supply no personal aggregate; they never select the machine-wide total.

A successful zero clears the badge. Before the gateway user list is known, own-count fallback stays unknown rather than guessing a shared reader. Legacy direct-engine subscriptions without a device retain their own count on single-reader installs.

This is also why `sw.js` needs no cross-workspace logic of its own: it mirrors `app_badge` verbatim, and iOS reads the same field natively.

**Re-assert, never diff (the bell and the icon must never disagree about THIS workspace).** The icon badge is an **externally written surface**, written while the page is backgrounded or closed. iOS writes it from the push payload's `app_badge` in its parent process, without running the page. The SW `push` handler writes it on Chrome/Android.

So the page must *re-assert* the count at every point where it (re)establishes the unread truth. Re-asserting is an unconditional write of `unreadCount` plus the other workspaces behind the gateway. `unreadCount` is the single source the bell badge and the Unread tab project from. The points are:

- `loadUnreadNotifications`, after applying a fresh set;
- the mark-read paths (`markReadOptimistic`, `markAllRead`, through `holdReads` and `abandonReads`);
- page resume (`onResume` in `store/startup.ts`), before the reload, so the icon matches the bell even when the engine is unreachable.

The `unreadCount` effect in `store/effects.ts` is *additionally* there for instant change-driven updates and the module-init clear. It cannot be the only writer: a computed whose recomputed value is equal notifies no subscriber. So a resume-time reload landing the same count re-runs nothing.

That gap left an iOS PWA showing **icon 1 / bell 0**. It followed a notification read on another device, or a tap that dropped a row this device never held. The count went 0 → 0 while the icon kept what the push wrote. A "skip the write when our count didn't change" optimization reintroduces it.

**Known boundary: a CLOSED PWA's icon can go stale.** Reads emit no push, and iOS only updates a web app's badge from a visible push. So a read on another device, while this PWA is closed, leaves its home-screen icon at the last pushed count. It corrects on the next push (which carries a fresh `app_badge`), or when the user opens or resumes the app (which re-asserts as above). Nothing on the engine side can close that window without sending a spurious visible notification.

**The aggregate is running workspaces only.** The gateway holds no DB handle (ADR 0014 §1), so it reads each count from a running engine over HTTP. It does not poll. One *row subscription* per engine follows that engine's row stream. It reads the count again on a notification, backup or preference event, a deleted thread and a users change (ADR 0756). A stream silent for three keepalive intervals is dropped and subscribed to again.

Personal listings and push totals use a stamped `GET /api/v1/notifications?limit=0` read as the inbox reader. For a picker listing the gateway holds that answer per device, until the next event that could move it. A push total never uses a held answer: it asks each engine every time. The shared count never substitutes for a failed personal read.

A stopped workspace reports no count and contributes 0. An engine built before the row stream shows no count on its picker row until it restarts. An unreachable healthy workspace makes the personal aggregate unknown.

A gateway install DOES navigate into a workspace, which is what the `scope: "/"` re-stamp is for. Its badge stays the total by construction rather than by confinement: the page adds the others to its own count, and a push carries the total for a gateway-scoped subscription. The Tauri count comes from a Rust loop independent of the loaded page. It likewise stays the total even when the webview is inside one workspace.

That desktop loop reads the aggregate, `/~/api/v1/control/unread-total`, without a recipient parameter. Authenticated local processes retain this machine-wide count for desktop compatibility. The gateway answers from memory: the sum of what each row subscription last read. A nudge can arrive before the gateway has read the count again after the same event. The periodic tick then corrects the badge.

**Tray title and dock badge** (macOS): the menu-bar tray-icon title carries the unread count at all times, so it never depends on a window being open. Closing all client windows drops the app to the menu-bar tray. That is the Accessory activation policy, out of the Dock and Cmd+Tab (see `crates/lucidos-app/src/activation.rs` `set_menu_bar_only` / `apply_unread_indicator`). There is no dock tile there, so the tray title is the only surface showing it. Reopening a window (Regular) brings the dock badge back alongside the unchanged tray title. `apply_unread_indicator` re-applies the last known count (`LAST_UNREAD`) on each transition, so the dock tile that just appeared or vanished agrees with the tray.

