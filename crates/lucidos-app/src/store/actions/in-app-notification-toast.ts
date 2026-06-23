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
import { dismissToast, showToast, toasts, focusedThreadId, threadMap, threadsLoaded, unreadNotifications, TOAST_AUTO_DISMISS_MS } from '../store';
import { isInViewport } from '../../utils/viewport';
import { isPageActive } from '../../utils/pageActive';
import { isIOSPwa } from '../../utils/platform';
import { postClientLog } from '../../utils/liveness';

const NOTIFICATION_TOAST_PREFIX = 'notification-';
const OVERFLOW_TOAST_KEY = 'notifications-overflow';
const MAX_INDIVIDUAL_TOASTS = 4;
const OVERFLOW_COUNT_PATTERN = /^\+(\d+) /;

/** PWA best-effort window (plan 2026-06-19-ios-native-apns-app Phase 0). On an
 *  iOS PWA the WebKit bug can swallow a push tap (it just focuses the app and
 *  drops the deep link). On resume we surface recent UNREAD navigate-kind
 *  notifications as a tappable in-app affordance so the deep link is still one
 *  in-app tap away. 24h covers an overnight notification tapped the next morning
 *  while excluding ancient unread; the unread set is already capped at ~100. */
const RESUME_AFFORDANCE_MAX_AGE_MS = 24 * 60 * 60 * 1000;

/** Session-scoped set of notification ids already surfaced in-app (by the §4
 *  toast OR a prior resume affordance), so the resume affordance never re-nags
 *  the same notification on every wake. Worked taps self-exclude separately:
 *  they mark the notification read, dropping it from the unread set. */
const surfacedNotificationIds = new Set<string>();

/** Mark a notification id as already surfaced in-app so the resume affordance
 *  (surfaceResumeNotificationAffordance) won't re-surface it. */
export function markNotificationSurfaced(id: string): void {
  surfacedNotificationIds.add(id);
}

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
  // The deep link is now being acted on (navigate / open modal / mark-read), so
  // the resume affordance must NOT also surface a toast for this same
  // notification. Stamp it surfaced here — a local check that doesn't depend on
  // the read-state round-trip. Without this, when an iOS tap DID deep-link, the
  // affordance's own unread reload could still see the row as unread (mark-read
  // POST not yet landed) and pop a redundant toast on top of the navigation.
  if (target.notification) markNotificationSurfaced(target.notification);
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

/** PWA best-effort deep-link rescue (plan 2026-06-19-ios-native-apns-app Phase 0).
 *
 *  Call on PWA resume / cold-start AFTER the unread set is (re)loaded. On iOS the
 *  WebKit bug can swallow a push tap — it just focuses the app and never delivers
 *  the deep link (no `notificationclick`, no declarative navigate applied). This
 *  surfaces a SINGLE still-unread navigate-kind notification as a tappable,
 *  dismissible in-app affordance whose tap routes through `dispatchDeepLink` — an
 *  in-app tap works even though the OS tap didn't.
 *
 *  Only the single-notification case is shown — it's the only one that can
 *  deep-link to a target. A backlog of 2+ fresh unread surfaces nothing (the
 *  bell badge already covers it); the old "+N more notifications" overflow was a
 *  recurring nag that reopened the inbox without ever resolving.
 *
 *  Strictly non-hijacking: it NEVER auto-navigates (navigate-on-resume would yank
 *  a user who reopened the app for an unrelated reason). It only ever shows a
 *  dismissible toast.
 *
 *  Self-de-duping:
 *   - A tap that DID work marks its notification read → it's not in the unread
 *     set → not surfaced.
 *   - `surfacedNotificationIds` stops the same unread row from re-nagging on every
 *     wake (and the §4 toast marks its rows surfaced too).
 *   - Only `tap.kind === 'navigate'` rows qualify — `modal`/`none` need no
 *     deep-link rescue (the bell/inbox already reaches them).
 *   - A 24h age cap keeps a cold load from surfacing ancient unread.
 *   - 2+ fresh unread surfaces nothing (no deep-link target to rescue). */
export function surfaceResumeNotificationAffordance(): void {
  // iOS standalone PWA ONLY. The swallowed-push-tap WebKit bug this rescues is
  // iOS-PWA-specific; everywhere else the OS push tap delivers the deep link
  // (notificationclick in the SW) and the bell badge already reflects unread.
  // Running it on a desktop browser was pure noise: clicking the affordance
  // opens the inbox / nothing but never marks the unread read, so it re-surfaced
  // on every reload (the "+N more notifications" toast that won't go away).
  if (!isIOSPwa()) return;
  const set = unreadNotifications.value;
  if (set.status !== 'loaded') {
    // Diagnostic (best-effort telemetry, no user intent): confirms Phase 0 ran
    // on the device and why it surfaced nothing — the affordance is otherwise
    // invisible in logs (see plan 2026-06-19-ios-native-apns-app Phase 0).
    postClientLog('deeplink', 'resume_affordance', { status: set.status, surfaced: 'none' });
    return;
  }
  const now = Date.now();
  const navigateUnread = set.data.filter(
    (n) => !n.read && (n.tap?.kind ?? 'modal') === 'navigate',
  ).length;
  const fresh = set.data.filter((n) => {
    if (n.read) return false;
    if ((n.tap?.kind ?? 'modal') !== 'navigate') return false;
    if (surfacedNotificationIds.has(n.id)) return false;
    const createdMs = new Date(n.created_at).getTime();
    if (!Number.isFinite(createdMs) || now - createdMs > RESUME_AFFORDANCE_MAX_AGE_MS) return false;
    return true;
  });
  // Diagnostic: total unread / navigate-kind unread / how many were fresh (not
  // already-surfaced + within the age cap) / what we surfaced. Tells us on the
  // next iOS tap whether Phase 0 fired and why it did or didn't show anything.
  // Only the single-fresh case surfaces a toast (the "+N more" overflow was
  // removed — see below), so 0 or 2+ fresh both report 'none'.
  postClientLog('deeplink', 'resume_affordance', {
    total_unread: set.data.length,
    navigate_unread: navigateUnread,
    fresh: fresh.length,
    surfaced: fresh.length === 1 ? 'single' : 'none',
  });
  // ONLY the single-notification case is surfaced. It's the only one that
  // delivers the actual rescue: tap → deep-link straight to the thread the
  // swallowed push tap should have opened. Two-or-more fresh unread can't
  // deep-link to one target, so the old "+N more notifications" overflow just
  // reopened the inbox (which the bell badge already does) WITHOUT marking
  // anything read — so it re-surfaced on every cold-start/resume: a recurring
  // nag with no rescue, and a confusing label standalone ("+2 more" — more than
  // what?). Drop it. The bell badge already surfaces a multi-notification
  // backlog. We do NOT stamp the 2+ set into surfacedNotificationIds, so if it
  // later drops to a single fresh unread, that one still gets its rescue toast.
  if (fresh.length !== 1) return;
  const n = fresh[0];
  surfacedNotificationIds.add(n.id);
  const target: DeepLinkTarget = {
    notification: n.id,
    thread: n.thread_id ?? null,
    event: n.event_id ?? null,
    tap: n.tap ?? null,
  };
  const toastKey = `${NOTIFICATION_TOAST_PREFIX}${n.id}`;
  let opened = false;
  const safeTitle = n.title && n.title.length > 0 ? n.title : 'Notification';
  showToast(n.message ? `${safeTitle}: ${n.message}` : safeTitle, 'info', {
    key: toastKey,
    onClick: () => {
      if (opened) return;
      opened = true;
      dismissToast(toastKey);
      dispatchDeepLink(target);
    },
  });
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
  // This notification is being surfaced in-app now, so the resume affordance
  // (surfaceResumeNotificationAffordance) must not re-surface it on the next wake.
  if (target.notification) markNotificationSurfaced(target.notification);
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
