
import { json, API } from '../../api/client/_core';
import type { Tap } from '@lucidos/sdk';
import { isTauri } from '../../utils/platform';
import { WORKSPACE_ID } from '../../utils/basePath';
import {
  showNativeNotification,
  dismissNativeNotification,
  takePendingNativeTaps,
  listen,
} from '../../utils/tauri';
import { dispatchDeepLink, presentInAppNotification } from './in-app-notification-toast';
import { parseDeepLinkFromSwMessage, type DeepLinkTarget } from './notification-deeplink';
import { postClientLog } from '../../utils/liveness';
import { isInAudience } from './push-audience';

export const NATIVE_PUSH_STALE_AFTER_MS = 5000;

export interface NativePushRequestedPayload {
  notification_id: string;
  title: string;
  body: string;
  thread_id?: string | null;
  event_id?: string | null;
  app_id?: string | null;
  tap?: Tap | null;
  sent_at_ms: number;
  audience?: string[] | null;
}

export function handleNativePushRequested(payload: NativePushRequestedPayload): void {
  if (!isTauri()) return;
  if (Date.now() - payload.sent_at_ms > NATIVE_PUSH_STALE_AFTER_MS) return;
  if (!isInAudience(payload.audience)) return;
  const outcome = presentInAppNotification({
    title: payload.title,
    body: payload.body,
    target: {
      notification: payload.notification_id,
      thread: payload.thread_id ?? null,
      event: payload.event_id ?? null,
      tap: payload.tap ?? null,
    },
  });
  if (outcome === 'not_shown') {
    void showNativeBanner(payload).then((accepted) => { if (accepted) void acknowledgeNativePush(payload.notification_id); });
  } else {
    void acknowledgeNativePush(payload.notification_id);
  }
}

async function acknowledgeNativePush(notificationId: string): Promise<void> {
  try {
    await json(`${API}/notification/native-receipt`, {
      method: 'POST', body: JSON.stringify({ notification_id: notificationId }),
      headers: { 'Content-Type': 'application/json' },
    });
  } catch (error) {
    console.warn('[NativePush] Receipt failed:', error);
  }
}

export interface NativePushDismissRequestedPayload {
  notification_id: string | null;
  sent_at_ms: number;
  audience?: string[] | null;
}

export function handleNativePushDismiss(payload: NativePushDismissRequestedPayload): void {
  if (!isTauri()) return;
  if (Date.now() - payload.sent_at_ms > NATIVE_PUSH_STALE_AFTER_MS) return;
  if (!isInAudience(payload.audience)) return;
  void dismissNativeBanner(payload);
}

async function dismissNativeBanner(payload: NativePushDismissRequestedPayload): Promise<void> {
  try {
    await dismissNativeNotification({
      workspace: WORKSPACE_ID,
      notificationId: payload.notification_id,
    });
  } catch (err) {
    // BEST-EFFORT: telemetry carve-out (.claude/rules/frontend.md). It runs on a
    // background SSE frame without user intent. A failed dismiss is non-fatal — the stale OS
    // banner just persists until the user swipes it (the pre-dismiss behaviour),
    // and the bell badge (NotificationRead SSE) already reflects the read state.
    // A toast would be wrong: the user isn't looking at the app. The durable
    // record is not this warn — `invoke` reports IPC failures to the engine log
    // itself (utils/ipcHealth → `[Client/ipc]`), which is what makes a dead
    // bridge visible on a packaged build that has no console.
    console.warn('[NativePush] Failed to dismiss native notification:', err);
  }
}

async function showNativeBanner(payload: NativePushRequestedPayload): Promise<boolean> {
  try {
    const title = payload.title.length > 0 ? payload.title : 'Lucidos';
    const accepted = await showNativeNotification({
      title,
      body: payload.body,
      deepLink: {
        notification_id: payload.notification_id,
        thread_id: payload.thread_id ?? null,
        event_id: payload.event_id ?? null,
        tap: payload.tap ?? null,
        workspace: WORKSPACE_ID,
      },
    });
    return accepted === true || accepted === null;
  } catch (err) {
    // BEST-EFFORT: telemetry carve-out (.claude/rules/frontend.md). It runs on a
    // background SSE frame without user intent. A failed native banner is non-fatal — the bell
    // badge (NotificationCreated) is the durable signal and the next
    // delivery worker retries. A toast here would be wrong: the user isn't
    // looking at the app (that's exactly why we chose the OS surface). The
    // failure is still recorded durably: `invoke` reports it to the engine log
    // (utils/ipcHealth → `[Client/ipc]`). Silence here is precisely how the
    // tauri 2.11 ACL regression hid — every banner was rejected for a month and
    // the only trace was a console warning nobody could see.
    console.warn('[NativePush] Failed to show native notification:', err);
    return false;
  }
}

export async function setupNativePushTapRouting(): Promise<() => void> {
  if (!isTauri()) return () => {};
  const drainAndDispatch = async (
    source: 'signal' | 'startup' | 'focus' | 'visibility',
  ): Promise<void> => {
    try {
      const taps = await takePendingNativeTaps(WORKSPACE_ID);
      if (taps.length === 0) return;
      const targets = taps
        .map((raw) => parseDeepLinkFromSwMessage(raw))
        .filter((target): target is DeepLinkTarget => target !== null);
      postClientLog('native-tap', 'drain', {
        source,
        count: taps.length,
        dispatched: targets.length,
      });
      for (const target of targets) dispatchDeepLink(target);
    } catch (err) {
      console.warn('[NativePush] Failed to drain pending taps:', err);
    }
  };

  const cleanups: Array<() => void> = [];

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

  try {
    const unlisten = await listen('native-notification-tapped', () => {
      void drainAndDispatch('signal');
    });
    cleanups.push(unlisten);
  } catch (err) {
    console.warn('[NativePush] Failed to register native tap listener:', err);
  }

  void drainAndDispatch('startup');

  return () => {
    for (const cleanup of cleanups) cleanup();
  };
}
