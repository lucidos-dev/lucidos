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
import { showNativeNotification, listen } from '../../utils/tauri';
import { dispatchDeepLink } from './in-app-notification-toast';
import { parseDeepLinkFromSwMessage } from './notification-deeplink';

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
    // looking at the app (that's exactly why we chose the OS surface).
    console.warn('[NativePush] Failed to show native notification:', err);
  }
}

/** Wire native-notification taps to the deep-link router. The Rust
 *  `show_native_notification` command emits `native-notification-tapped` with
 *  the SW-message-shaped deep link when the user clicks a banner; route it
 *  through the SAME `dispatchDeepLink` the web-push SW tap uses, so a native tap
 *  marks-read + navigates identically to a web-push tap. Tauri-only; returns an
 *  unlisten. Call once at startup. See system-knowhow/notifications.md §4. */
export async function setupNativePushTapRouting(): Promise<() => void> {
  if (!isTauri()) return () => {};
  return listen<Record<string, unknown>>('native-notification-tapped', (e) => {
    const target = parseDeepLinkFromSwMessage(e.payload);
    if (target) dispatchDeepLink(target);
  });
}
