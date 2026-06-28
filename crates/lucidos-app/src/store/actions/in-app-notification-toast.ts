// The §4 matrix of system-knowhow/notifications.md. Driven by the engine's
// `NotificationToastRequested` SSE (handleNotificationToastRequested below),
// which the engine emits ONLY after it decides to suppress the OS push — so a
// toast and a push can never both fire for one notification. We do NOT fire
// from NotificationCreated (that flushes from the iOS PWA SSE queue after a
// push tap and would leak a duplicate toast).

import type { Tap } from '@lucidos/sdk';
import { markReadOptimistic, viewNotification } from './notifications';
import { switchMenuItem } from './menu';
import { handleNavigationRequest } from './thread-sync';
import {
  resolveDeepLink,
  type DeepLinkTarget,
} from './notification-deeplink';
import { dismissToast, showToast, toasts, focusedThreadId, threadMap, threadsLoaded, TOAST_AUTO_DISMISS_MS } from '../store';
import { isInViewport } from '../../utils/viewport';
import { isPageActive } from '../../utils/pageActive';
import { postClientLog } from '../../utils/liveness';

const NOTIFICATION_TOAST_PREFIX = 'notification-';
const OVERFLOW_TOAST_KEY = 'notifications-overflow';
const MAX_INDIVIDUAL_TOASTS = 4;
const OVERFLOW_COUNT_PATTERN = /^\+(\d+) /;

/** Spec §4 row labels. Row 5 (offline) doesn't apply — we only run when
 *  an SSE event landed, meaning the page is online by definition. */
export type InAppRow = 'row1_auto_read' | 'row2_or_3_toast_and_badge' | 'row4_hidden';

/** Apply the §4 matrix to a notification's deep-link target. */
export function classifyInAppRow(target: DeepLinkTarget): InAppRow {
  if (!isPageActive()) return 'row4_hidden';
  // Row 1 requires a non-null event_id per spec §2 — when null the device
  // falls through to Row 2 (if thread matches) or Row 3.
  if (
    target.thread &&
    target.event &&
    focusedThreadId.value === target.thread &&
    isInViewport(target.event)
  ) {
    return 'row1_auto_read';
  }
  return 'row2_or_3_toast_and_badge';
}

/** Route a deep-link target to the matching dispatcher action. Returns true
 *  when the link resolved to a non-noop action.
 *
 *  Mark-read is universal across kinds: modal (via viewNotification's
 *  internal mark), none (explicit), and navigate (here, in parallel with
 *  the navigation). The source notification id always flips to read on tap. */
export function dispatchDeepLink(target: DeepLinkTarget): boolean {
  const action = resolveDeepLink(target);
  // Diagnostic breadcrumb (best-effort telemetry, no user intent): records which
  // dispatcher branch ran for a tap. For navigate-kind it also records whether
  // the destination thread is ALREADY in the loaded map — at cold-start (iOS
  // push reload) it usually isn't, so focusThreadOrBootstrap takes the async
  // bootstrap-fetch path, which is the fragile point on iOS (suspended timers /
  // hung fetches). Lets "marked read but never navigated" be pinned to the cause.
  postClientLog('deeplink', 'dispatch', {
    action: action.type,
    target: action.type === 'navigate' ? action.to.target : null,
    thread_in_map:
      action.type === 'navigate' && action.to.target === 'thread' && action.to.id
        ? threadMap.value.has(action.to.id)
        : null,
    threads_loaded: threadsLoaded.value,
  });
  switch (action.type) {
    case 'navigate':
      if (action.notification) markReadOptimistic(action.notification);
      handleNavigationRequest(action.to);
      return true;
    case 'view-notification':
      // viewNotification opens the detail in the content pane AND marks the row
      // read. It's async (fetches the full row), but the open + mark-read are
      // best-effort from the dispatcher's POV: `viewNotification` owns its own
      // failure toast on the GET, so `void` here keeps the discriminated return
      // synchronous without dropping the error path.
      void viewNotification(action.id);
      return true;
    case 'mark-read':
      markReadOptimistic(action.id);
      return true;
    case 'noop':
      return false;
  }
}

interface InAppNotificationToastInput {
  title: string;
  body: string;
  target: DeepLinkTarget;
}

export function showInAppNotificationToast({ title, body, target }: InAppNotificationToastInput): void {
  const row = classifyInAppRow(target);

  if (row === 'row4_hidden') {
    // Bell badge updates via the parent SSE dispatch's handleNotificationSSE
    // (loadUnreadNotifications). Suppress the toast — by the time the user
    // resumes the PWA it's stale and sits on top of a deep-link landing
    // (see work-tracker `pwa-stale-sticky-notification-toast-on-resume`).
    return;
  }

  if (row === 'row1_auto_read') {
    // User is literally looking at the source event. Mark read — which drops it
    // from the unread set so the badge never bumps — and skip the toast. The
    // optimistic removal invalidates the in-flight set reload handleNotificationSSE
    // kicked off, so the created notification can't briefly surface on the badge.
    if (target.notification) markReadOptimistic(target.notification);
    return;
  }

  // Row 2 or 3: active page, on a different thread OR the same thread
  // scrolled away from the source event. Toast + badge (badge already
  // bumped by handleNotificationSSE in the parent dispatch).
  //
  // The toast is ambient (see notifications.md §4) — an Open button is only
  // meaningful when the deep link actually navigates somewhere the toast
  // itself doesn't already show. A `view-notification` action would open the
  // detail panel for the same title + body the toast already renders.
  const resolved = resolveDeepLink(target);
  const hasNavigationTarget = resolved.type === 'navigate';
  // tap.kind === 'none': the notification is passive by contract — no
  // follow-up required. Mark read immediately, BEFORE the overflow guard,
  // so a pile-up doesn't leave passive rows sitting unread in the inbox
  // waiting for an acknowledgement that will never come. notifications.md §4
  // makes this explicit: "the row IS read the moment the user could have
  // seen it".
  if (resolved.type === 'mark-read') {
    markReadOptimistic(resolved.id);
  }

  let individualCount = 0;
  let currentOverflow = 0;
  for (const t of toasts.value) {
    if (t.key === OVERFLOW_TOAST_KEY) {
      const m = t.message.match(OVERFLOW_COUNT_PATTERN);
      currentOverflow = m ? parseInt(m[1], 10) : 0;
    } else if (t.key?.startsWith(NOTIFICATION_TOAST_PREFIX)) {
      individualCount++;
    }
  }

  if (currentOverflow > 0 || individualCount >= MAX_INDIVIDUAL_TOASTS) {
    showOverflowToast(currentOverflow + 1);
    return;
  }

  const safeTitle = title.length > 0 ? title : 'Lucidos';
  const message = body ? `${safeTitle}: ${body}` : safeTitle;
  const toastKey = target.notification ? `${NOTIFICATION_TOAST_PREFIX}${target.notification}` : undefined;
  // Toast.tsx fires onClick raw and the DOM stays mounted across the async
  // dismiss render, so a quick second tap would re-run dispatchDeepLink —
  // and handleNavigationRequest's branches (openAppById /
  // focusThreadOrBootstrap) aren't both idempotent. Once-only flag +
  // dismiss-first prevents it.
  let opened = false;
  const onClick = hasNavigationTarget
    ? () => {
        if (opened) return;
        opened = true;
        if (toastKey) dismissToast(toastKey);
        dispatchDeepLink(target);
      }
    : undefined;
  // tap.kind === 'none' has no click target to wait on; opt back into
  // auto-dismiss.
  const autoDismissMs = resolved.type === 'mark-read' ? TOAST_AUTO_DISMISS_MS : undefined;
  showToast(message, 'info', { key: toastKey, onClick, autoDismissMs });
}

/** Wall-clock budget after which a `NotificationToastRequested` is too stale
 *  to render. The engine emits this only on the push-suppressed branch, so
 *  there is no OS push to collide with — but a toast that flushes seconds
 *  late from the iOS PWA SSE queue (after the user resumed the PWA) would sit
 *  on top of whatever they're now doing. Drop it; the bell badge (driven by
 *  NotificationCreated) already reflects the notification. Sized to
 *  comfortably cover PresenceCheck's deadline (`DEADLINE_MS` = 2s) plus a
 *  pong round-trip so a legitimately just-decided toast always renders.
 *  Exported so tests assert against the same constant. */
export const TOAST_REQUEST_STALE_AFTER_MS = 5000;

export interface NotificationToastRequestedPayload {
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

/** SSE handler for the engine's §4 in-app toast trigger. Fired after the
 *  engine suppresses the OS push (an active device pong'd in). Delegates to
 *  showInAppNotificationToast, which applies the §4 row matrix: render the
 *  toast (Row 2/3), auto-read silently (Row 1), or no-op (Row 4 hidden). See
 *  system-knowhow/notifications.md §4. */
export function handleNotificationToastRequested(payload: NotificationToastRequestedPayload): void {
  if (Date.now() - payload.sent_at_ms > TOAST_REQUEST_STALE_AFTER_MS) {
    return;
  }
  showInAppNotificationToast({
    title: payload.title,
    body: payload.body,
    target: {
      notification: payload.notification_id,
      thread: payload.thread_id ?? null,
      event: payload.event_id ?? null,
      tap: payload.tap ?? null,
    },
  });
}

function showOverflowToast(count: number): void {
  const noun = count === 1 ? 'notification' : 'notifications';
  let opened = false;
  showToast(`+${count} more ${noun}`, 'info', {
    key: OVERFLOW_TOAST_KEY,
    onClick: () => {
      if (opened) return;
      opened = true;
      dismissToast(OVERFLOW_TOAST_KEY);
      switchMenuItem('notifications');
    },
  });
}
