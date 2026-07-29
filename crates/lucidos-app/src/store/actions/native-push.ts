// The native desktop counterpart of the OS push (system-knowhow/notifications.md
// §1, §4). Driven by the engine's `NativePushRequested` SSE, which the engine
// emits ONLY on the push-ALLOWED branch (no active device) — the mutually
// exclusive complement of `NotificationToastRequested` (push-suppressed branch,
// in-app toast). A Tauri desktop app embeds a WKWebView that can't subscribe to
// Web Push, so the engine reaches it over the already-open SSE stream and the
// page renders a native macOS banner via the `show_native_notification` command
// (Apple's UserNotifications framework / UNUserNotificationCenter, in
// notifications.rs). Tapping the banner routes through the SAME dispatchDeepLink
// the web-push service-worker tap uses.

import type { Tap } from '@lucidos/sdk';
import { isTauri } from '../../utils/platform';
import { isPageActive } from '../../utils/pageActive';
import {
  showNativeNotification,
  dismissNativeNotification,
  takePendingNativeTaps,
  listen,
} from '../../utils/tauri';
import { dispatchDeepLink } from './in-app-notification-toast';
import { parseDeepLinkFromSwMessage } from './notification-deeplink';
import { postClientLog } from '../../utils/liveness';

/** Wall-clock budget after which a `NativePushRequested` is too stale to show.
 *  Mirrors `TOAST_REQUEST_STALE_AFTER_MS`: the engine emits this only after the
 *  PresenceCheck resolves, so a frame that flushes seconds late from a
 *  suspended-tab SSE queue would pop a banner long after the moment passed —
 *  drop it (the bell badge, driven by NotificationCreated, still reflects it).
 *  Exported so tests assert against the same constant. */
export const NATIVE_PUSH_STALE_AFTER_MS = 5000;

export interface NativePushRequestedPayload {
  notification_id: string;
  title: string;
  body: string;
  thread_id?: string | null;
  event_id?: string | null;
  app_id?: string | null;
  tap?: Tap | null;
  /** Engine wall-clock at emit time. Drives the freshness gate. */
  sent_at_ms: number;
}

/** SSE handler for the engine's native desktop surface trigger. Gates before
 *  touching the OS:
 *   - not Tauri → no-op (browser / PWA get the real web push instead);
 *   - stale frame → no-op (late SSE-queue flush);
 *   - page active → no-op (OS surface is for non-active devices; macOS
 *     suppresses banners for the frontmost app anyway).
 *  See system-knowhow/notifications.md §4. */
export function handleNativePushRequested(payload: NativePushRequestedPayload): void {
  if (!isTauri()) return;
  if (Date.now() - payload.sent_at_ms > NATIVE_PUSH_STALE_AFTER_MS) return;
  if (isPageActive()) return;
  void showNativeBanner(payload);
}

/** Payload of the engine's `NativePushDismissRequested` SSE — the cross-device
 *  dismiss counterpart of `NativePushRequested`. `notification_id` is `null` for
 *  a dismiss-all (the mark-all-read path), otherwise the one banner to remove. */
export interface NativePushDismissRequestedPayload {
  notification_id: string | null;
  /** Engine wall-clock at emit time. Drives the freshness gate. */
  sent_at_ms: number;
}

/** SSE handler for the engine's cross-device dismiss trigger. When a
 *  notification is read on any device, the engine broadcasts this so a connected
 *  Tauri desktop app REMOVES the already-delivered native banner(s). Gates:
 *   - not Tauri → no-op (the open web can't silently remove a Web Push banner —
 *     Safari revokes a subscription after 3 silent pushes — so browser / PWA
 *     banners persist until manually swiped; in-app unread still syncs via
 *     `NotificationRead`);
 *   - stale frame → no-op (a late SSE-queue flush; this bounds — does not fully
 *     eliminate, since `removeAllDeliveredNotifications` is blunt — the window in
 *     which a late dismiss-all could clear a banner for a notification created
 *     after the all-read).
 *  NO `isPageActive()` gate (unlike show): a banner exists only because the
 *  device was inactive when shown; by dismiss time it may be active again, and
 *  removeDeliveredNotifications is a harmless no-op when nothing matches.
 *  See system-knowhow/notifications.md §4. */
export function handleNativePushDismiss(payload: NativePushDismissRequestedPayload): void {
  if (!isTauri()) return;
  if (Date.now() - payload.sent_at_ms > NATIVE_PUSH_STALE_AFTER_MS) return;
  void dismissNativeBanner(payload);
}

async function dismissNativeBanner(payload: NativePushDismissRequestedPayload): Promise<void> {
  try {
    await dismissNativeNotification({ notificationId: payload.notification_id });
  } catch (err) {
    // Telemetry carve-out (.claude/rules/frontend.md): runs on a background SSE
    // frame without user intent. A failed dismiss is non-fatal — the stale OS
    // banner just persists until the user swipes it (the pre-dismiss behaviour),
    // and the bell badge (NotificationRead SSE) already reflects the read state.
    // A toast would be wrong: the user isn't looking at the app. The durable
    // record is not this warn — `invoke` reports IPC failures to the engine log
    // itself (utils/ipcHealth → `[Client/ipc]`), which is what makes a dead
    // bridge visible on a packaged build that has no console.
    console.warn('[NativePush] Failed to dismiss native notification:', err);
  }
}

async function showNativeBanner(payload: NativePushRequestedPayload): Promise<void> {
  try {
    const title = payload.title.length > 0 ? payload.title : 'Lucidos';
    await showNativeNotification({
      title,
      body: payload.body,
      // SW-message shape so the tap routes through the same dispatchDeepLink the
      // web-push tap uses (parseDeepLinkFromSwMessage). `app_id` is omitted on
      // purpose — when the tap navigates to an app, the id lives in tap.to.app_id.
      deepLink: {
        notification_id: payload.notification_id,
        thread_id: payload.thread_id ?? null,
        event_id: payload.event_id ?? null,
        tap: payload.tap ?? null,
      },
    });
  } catch (err) {
    // Telemetry carve-out (.claude/rules/frontend.md): runs on a background SSE
    // frame without user intent. A failed native banner is non-fatal — the bell
    // badge (NotificationCreated) is the durable signal and the next
    // notification re-attempts. A toast here would be wrong: the user isn't
    // looking at the app (that's exactly why we chose the OS surface). The
    // failure is still recorded durably: `invoke` reports it to the engine log
    // (utils/ipcHealth → `[Client/ipc]`). Silence here is precisely how the
    // tauri 2.11 ACL regression hid — every banner was rejected for a month and
    // the only trace was a console warning nobody could see.
    console.warn('[NativePush] Failed to show native notification:', err);
  }
}

/** Wire native-notification taps to the deep-link router, routed through the
 *  SAME `dispatchDeepLink` the web-push SW tap uses (so a native tap marks-read +
 *  navigates identically — by default opening the notification-detail modal, the
 *  `Tap::Modal` default).
 *
 *  DURABLE delivery: the deep link is NOT read off the live event payload. The
 *  Rust delegate stashes every tapped deep link and emits
 *  `native-notification-tapped` only as a wake signal; we DRAIN the stash
 *  (`takePendingNativeTaps`) on FOUR triggers — startup (cold), the live signal
 *  (warm), and the window regaining focus / visibility. This is the fix for taps
 *  lost when the page wasn't listening at emit time — webview reloaded /
 *  suspended-while-trayed / client relaunched — which Tauri does not replay.
 *
 *  Why focus/visibility matter: a native banner only shows while the page is
 *  INACTIVE (handleNativePushRequested gates on isPageActive), so a banner tap
 *  ALWAYS lands on a backgrounded/hidden — often JS-throttled — WKWebView.
 *  `show_main_window` then SHOWS the existing window WITHOUT reloading, so the
 *  startup cold drain never re-runs, and the warm `app.emit` is a fire-and-forget
 *  eval that WebKit can drop onto a just-resumed webview. The `focus` /
 *  `visibilitychange` events WebKit dispatches IN the webview when the window is
 *  brought forward are eval-independent, so they reliably catch the stash the
 *  warm signal missed (the "tap focuses the window but never deep-links" bug).
 *  The Rust drain is atomic, so every tap routes exactly once across all triggers
 *  and never re-fires on a later reload. Tauri-only; returns an unlisten that
 *  removes every trigger. Call once at startup. See notifications.rs +
 *  system-knowhow/notifications.md §4. */
export async function setupNativePushTapRouting(): Promise<() => void> {
  if (!isTauri()) return () => {};
  const drainAndDispatch = async (
    source: 'signal' | 'startup' | 'focus' | 'visibility',
  ): Promise<void> => {
    try {
      const taps = await takePendingNativeTaps();
      if (taps.length > 0) {
        postClientLog('native-tap', 'drain', { source, count: taps.length });
      }
      for (const raw of taps) {
        const target = parseDeepLinkFromSwMessage(raw);
        if (target) dispatchDeepLink(target);
      }
    } catch (err) {
      // Telemetry carve-out (.claude/rules/frontend.md): runs at startup or on a
      // background signal, no user intent. A failed drain leaves taps stashed for
      // the next signal / reload; the bell badge (NotificationCreated) still
      // reflects the notification. A toast would be wrong — the user isn't looking.
      console.warn('[NativePush] Failed to drain pending taps:', err);
    }
  };

  const cleanups: Array<() => void> = [];

  // Reliable, eval-independent triggers: WebKit fires `focus` when the window
  // becomes key and `visibilitychange` when it un-occludes — both happen when a
  // banner tap brings the window forward. Registered BEFORE the warm listener so
  // a `listen` failure can't strand them.
  const onFocus = (): void => {
    void drainAndDispatch('focus');
  };
  const onVisibility = (): void => {
    if (document.visibilityState === 'visible') void drainAndDispatch('visibility');
  };
  window.addEventListener('focus', onFocus);
  document.addEventListener('visibilitychange', onVisibility);
  cleanups.push(() => window.removeEventListener('focus', onFocus));
  cleanups.push(() => document.removeEventListener('visibilitychange', onVisibility));

  // Warm path: the live wake signal. Best-effort — its eval can be dropped on a
  // suspended/just-resumed webview (the case the focus/visibility drains cover),
  // so a registration failure must not prevent those drains or the startup drain.
  try {
    const unlisten = await listen('native-notification-tapped', () => {
      void drainAndDispatch('signal');
    });
    cleanups.push(unlisten);
  } catch (err) {
    // Telemetry carve-out (.claude/rules/frontend.md): listener registration runs
    // at startup with no user intent; the focus/visibility drains still deliver.
    console.warn('[NativePush] Failed to register native tap listener:', err);
  }

  // Cold path: route any tap that arrived before this ran — the exact race the
  // durable Rust-side stash exists for. Runs after the triggers are wired so a
  // tap landing mid-setup is covered by whichever drain wins (the atomic take
  // guarantees it isn't dispatched twice).
  void drainAndDispatch('startup');

  return () => {
    for (const cleanup of cleanups) cleanup();
  };
}
