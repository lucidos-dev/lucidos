// The §4 matrix of system-knowhow/notifications.md. Driven by the engine's
// `NotificationToastRequested` SSE (handleNotificationToastRequested below),
// which the engine emits ONLY after it decides to suppress the OS push — so a
// toast and a push can never both fire for one notification. We do NOT fire
// from NotificationCreated (that flushes from the iOS PWA SSE queue after a
// push tap and would leak a duplicate toast).

import type { Tap } from '@lucidos/sdk';
import type { ToastAction } from '../types';
import { markReadOptimistic, viewNotification } from './notifications';
import { switchMenuItem } from './menu';
import { handleNavigationRequest } from './thread-sync';
import {
  resolveDeepLink,
  type DeepLinkTarget,
} from './notification-deeplink';
import { composeToastMessage } from '../../components/shared/toastMessage';
import { dismissToast, showToast, toasts, focusedThreadId, threadMap, threadsLoaded } from '../store';
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
 *  Every notification is openable, so the source notification id always flips
 *  to read on tap: `view-notification` (modal) marks read via viewNotification's
 *  internal mark; `navigate` marks it here, in parallel with the navigation. */
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
  const resolved = resolveDeepLink(target);
  const notifId = target.notification ?? null;

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
  // The title is the toast's HEADING, never glued onto the body's first line —
  // see composeToastMessage for why a structured body has to start on line 2.
  const message = composeToastMessage(safeTitle, body);
  const toastKey = notifId ? `${NOTIFICATION_TOAST_PREFIX}${notifId}` : undefined;

  // A `modal` or `navigate` notification gets a single [Open] button (rendered
  // right / primary via the Toast component's `action`) plus the toast's built-in
  // X (close). [Open] runs the deep link — opening the notification detail in the
  // content pane for `modal`, navigating to the destination for `navigate` — and
  // marks the notification read, so a toast the user has acted on never has to be
  // re-read in the Notifications panel. The X dismisses WITHOUT marking read,
  // deferring the row to the bell badge + panel for later. There is deliberately
  // no separate "OK / acknowledge" button: on a toast that HAS somewhere to go,
  // marking read without opening is a footgun (it would bury an unanswered
  // question), so the two meaningful outcomes are act-now ([Open]) or defer (X).
  // The `acted` guard: Toast.tsx fires onClick raw and the DOM lingers across the
  // async dismiss render, so a quick double-tap must not re-run a non-idempotent
  // open/navigate (dispatchDeepLink → openAppById / focusThreadOrBootstrap).
  // Dismiss-first, then act.
  let action: ToastAction | undefined;
  if (resolved.type === 'view-notification' || resolved.type === 'navigate') {
    let acted = false;
    action = {
      label: 'Open',
      onClick: () => {
        if (acted) return;
        acted = true;
        if (toastKey) dismissToast(toastKey);
        if (resolved.type === 'view-notification') {
          // modal: viewNotification marks read only AFTER the detail fetch
          // succeeds. An explicit [Open] must mark read even if that fetch fails
          // (else a dismissed toast leaves the row unread). Mark AFTER
          // viewNotification settles so we never race its own cold-open
          // load-before-mark ordering; markReadOptimistic is idempotent, so the
          // happy-path double is a no-op beyond one extra (idempotent) read POST.
          const id = resolved.id;
          const ensureRead = () => markReadOptimistic(id);
          void viewNotification(id).then(ensureRead, ensureRead);
        } else {
          dispatchDeepLink(target);
        }
      },
    };
  }

  // Notification toasts persist (no auto-dismiss) so their [Open] button (and the
  // X) stay usable; the user drives dismissal. There is no passive/button-less
  // kind anymore — every notification is openable. noAutofocus: these pop
  // unsolicited, so they must not steal keyboard focus (a reflexive Enter on
  // [Open] would navigate/open a notification the user never chose to act on).
  showToast(message, 'info', { key: toastKey, action, noAutofocus: true });
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
