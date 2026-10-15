// The §4 matrix of system-knowhow/notifications.md. Driven by the engine's
// `PresenceCheck` SSE (presence-pong.ts), which carries the toast. The page
// acts here FIRST and then reports what it did, and the engine suppresses the
// OS push only on that report. So a page that hides after answering still
// leaves the reader the toast it answered for. The native desktop frame
// (`NativePushRequested`, native-push.ts) comes through here too, when its
// window came forward since it answered. We do NOT fire from
// NotificationCreated (that flushes from the iOS PWA SSE queue after a push
// tap and would leak a duplicate toast).
//
// It owns the toast's whole lifetime, arrival to removal. A notification toast
// lives exactly as long as its row is unread on this page. That is what
// `installNotificationToastLifetime` at the foot of this file enforces.

import { effect, untracked } from '@preact/signals';
import type { PresenceOutcome } from '@lucidos/event-stream';
import type { Loadable, Notification } from '../types';
import { markReadOptimistic, viewNotification } from './notifications';
import { switchMenuItem } from './menu';
// Imported from the module that DEFINES it, not from `thread-sync` (which only
// re-exports it). `thread-sync` imports this file for the toast handler, so
// going back through it would close a needless module cycle between the two:
// the exact shape `.claude/rules/frontend.md` warns about, where whichever side
// initializes first sees the other's binding still undefined.
import { handleNavigationRequest } from './navigation-request';
import {
  resolveDeepLink,
  type DeepLinkTarget,
} from './notification-deeplink';
import { currentNotificationToasts } from './preferences';
import { appFullscreenHost } from '../appFullscreenHost';
import {
  appPseudoFullscreen,
  dismissToast,
  focusedThreadId,
  removeToast,
  showToast,
  threadMap,
  threadsLoaded,
  toasts,
  unreadNotifications,
  workspaceUnavailable,
} from '../store';
import { isInViewport } from '../../utils/viewport';
import { isPageActive } from '../../utils/pageActive';
import { repaintLandedContent } from '../../utils/pageResume';
import { postClientLog } from '../../utils/liveness';

const NOTIFICATION_TOAST_PREFIX = 'notification-';
const OVERFLOW_TOAST_KEY = 'notifications-overflow';
const MAX_INDIVIDUAL_TOASTS = 4;

/** The key a notification's own toast carries. One speller, so the arrival
 *  that raises the toast and the read that drops it cannot disagree about
 *  which one they mean. Same discipline as `visitKeys.ts` for the seen rule. */
export function notificationToastKey(id: string): string {
  return `${NOTIFICATION_TOAST_PREFIX}${id}`;
}

/** Whether the SHELL is the surface in front of the user.
 *
 *  Only the shell can show a toast. An app filling the screen means they are
 *  looking at the app, so the notification belongs to the OS.
 *
 *  That also makes a fullscreen app agree with a popped-out app window. The two
 *  look identical to the user, and a popped-out one has always taken the push.
 *
 *  Pure over its three inputs, so it is testable without a DOM. */
export function isShellActive(
  pageActive: boolean,
  nativeFullscreen: boolean,
  pseudoFullscreen: boolean,
): boolean {
  return pageActive && !nativeFullscreen && !pseudoFullscreen;
}

/** `isShellActive` against the live signals. */
export function shellIsActive(): boolean {
  return isShellActive(
    isPageActive(),
    appFullscreenHost.value !== null,
    appPseudoFullscreen.value,
  );
}

/** Spec §4 row labels. Row 5 (offline) doesn't apply — we only run when
 *  an SSE event landed, meaning the page is online by definition. The shell
 *  not being in front covers Row 4 (hidden) and an app filling the screen. */
export type InAppRow = 'row1_auto_read' | 'row2_or_3_toast_and_badge' | 'shell_not_active';

/** Apply the §4 matrix to a notification's deep-link target. */
export function classifyInAppRow(target: DeepLinkTarget): InAppRow {
  if (!shellIsActive()) return 'shell_not_active';
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
      return dispatched();
    case 'view-notification':
      // viewNotification opens the detail in the content pane AND marks the row
      // read. It's async (fetches the full row), but the open + mark-read are
      // best-effort from the dispatcher's POV: `viewNotification` owns its own
      // failure toast on the GET, so `void` here keeps the discriminated return
      // synchronous without dropping the error path.
      void viewNotification(action.id);
      return dispatched();
    case 'noop':
      return false;
  }
}

/** Close every branch that LANDED something, and answer the non-noop.
 *
 *  The tap that got here usually woke the page: a native banner only shows while
 *  it is inactive, so the tap lands on a webview WebKit has parked. The wake
 *  repaint has already run, against the view this one is replacing. So the
 *  arriving view asks for its own, AFTER the navigation rather than before it.
 *  Off WebKit this is free. See utils/pageResume.ts.
 *
 *  Called from each arm rather than after the switch, so every arm still
 *  RETURNS. That is what keeps the union exhaustiveness-checked: a fourth
 *  `DeepLinkAction` with no case here is a compile error rather than a silent
 *  no-op that repaints and reports success. */
function dispatched(): boolean {
  repaintLandedContent();
  return true;
}

/** A notification as the engine offers it to the page, ready to show. */
export interface InAppNotification {
  title: string;
  body: string;
  target: DeepLinkTarget;
}

/** Decide what this page does with a notification, do it, and say what it did.
 *
 *  One synchronous step, so the answer can never describe a page that has
 *  since changed: the toast is on screen before the caller reports `shown`. */
export function presentInAppNotification({ title, body, target }: InAppNotification): PresenceOutcome {
  // While the workspace is unavailable `showToast` draws nothing, so this page
  // cannot honestly report `shown`. The OS push is the surface then.
  if (workspaceUnavailable()) return 'not_shown';
  const row = classifyInAppRow(target);

  if (row === 'shell_not_active') return 'not_shown';

  if (row === 'row1_auto_read') {
    // User is literally looking at the source event. Mark read — which drops it
    // from the unread set so the badge never bumps — and skip the toast. The
    // optimistic removal invalidates the in-flight set reload handleNotificationSSE
    // kicked off, so the created notification can't briefly surface on the badge.
    if (target.notification) markReadOptimistic(target.notification);
    return 'auto_read';
  }

  // The user has turned in-app toasts off, so the notification waits on the
  // bell badge and in the Notifications panel instead of interrupting. It still
  // suppresses the push: the preference is about interruption, not presence,
  // and no toast AND a push would be the opposite of what was asked. Not in
  // `showToast`: an apply-change or error toast answers something the user just
  // did, and this silences only the unsolicited kind. Below the Row 1 branch on
  // purpose, so reading the source event still marks it read.
  if (!currentNotificationToasts()) return 'silenced';

  // Row 2 or 3: active page, on a different thread OR the same thread
  // scrolled away from the source event. Toast + badge (badge already
  // bumped by handleNotificationSSE in the parent dispatch).
  const resolved = resolveDeepLink(target);
  const notifId = target.notification ?? null;

  const individualCount = toasts.value
    .filter((t) => t.key?.startsWith(NOTIFICATION_TOAST_PREFIX)).length;

  if (overflowIsShowing() || individualCount >= MAX_INDIVIDUAL_TOASTS) {
    foldIntoOverflow(notifId);
    return 'shown';
  }

  const safeTitle = title.length > 0 ? title : 'Lucidos';
  const toastKey = notifId ? notificationToastKey(notifId) : undefined;

  // A `modal` or `navigate` notification opens on a tap anywhere on the card,
  // beside the toast's built-in X. The tap runs the deep link (the detail in the
  // content pane for `modal`, the destination for `navigate`) and marks the
  // notification read. The X dismisses WITHOUT marking read, leaving the row on
  // the bell badge and in the panel. There is deliberately no "OK" button:
  // marking read without opening would bury an unanswered question.
  // The `acted` guard: the DOM lingers across the async dismiss render, so a
  // quick double-tap must not re-run a non-idempotent open or navigate.
  // Dismiss first, then act.
  let onClick: (() => void) | undefined;
  if (resolved.type === 'view-notification' || resolved.type === 'navigate') {
    let acted = false;
    onClick = () => {
      if (acted) return;
      acted = true;
      if (toastKey) dismissToast(toastKey);
      if (resolved.type === 'view-notification') {
        // modal: viewNotification marks read only AFTER the detail fetch
        // succeeds. A tap must mark read even if that fetch fails, or a
        // dismissed toast leaves the row unread. Marking after it settles never
        // races its own load-before-mark ordering. markReadOptimistic is
        // idempotent, so the happy path costs one extra read POST.
        const id = resolved.id;
        const ensureRead = () => markReadOptimistic(id);
        void viewNotification(id).then(ensureRead, ensureRead);
      } else {
        dispatchDeepLink(target);
      }
    };
  }

  // Notification toasts persist (no auto-dismiss); the user drives dismissal.
  // The notification's title and body map straight onto the toast's. A
  // body-less one is its title alone.
  const [toastTitle, message] = body ? [safeTitle, body] : [undefined, safeTitle];
  showToast(message, 'info', { title: toastTitle, key: toastKey, onClick });
  return 'shown';
}

/** The notifications the overflow toast stands for.
 *
 *  It holds ids rather than a bare count, so a read can decrement it. A number
 *  parsed back out of the rendered message could not. Authoritative only while
 *  that toast is showing: a fold arriving with nothing on screen starts a fresh
 *  pile, so a count the reader has cleared never returns. */
const foldedNotifications = new Set<string>();
let anonymousFolds = 0;

function overflowIsShowing(): boolean {
  return toasts.value.some((t) => t.key === OVERFLOW_TOAST_KEY);
}

function foldIntoOverflow(notifId: string | null): void {
  if (!overflowIsShowing()) foldedNotifications.clear();
  // An id-less notification gets an unkeyed toast, so no read can ever reach
  // it. Give it a token nothing matches; only the count needs it to be there.
  foldedNotifications.add(notifId ?? `anonymous-${++anonymousFolds}`);
  showOverflowToast();
}

/** Drop one notification from the pile, and the toast with the last of them. */
function unfoldFromOverflow(id: string): void {
  if (!overflowIsShowing()) {
    // A cleared pile stays cleared. `showToast` RAISES a toast for a key that
    // is not on screen, so decrementing here would put the overflow toast back
    // up. A read that raises a toast is the inverse of the rule. The route in
    // is the toast's own tap: it dismisses and opens the Notifications panel,
    // where reading a folded row is the next thing the reader does.
    foldedNotifications.clear();
    return;
  }
  if (!foldedNotifications.delete(id)) return;
  if (foldedNotifications.size === 0) removeToast(OVERFLOW_TOAST_KEY);
  else showOverflowToast();
}

function showOverflowToast(): void {
  const count = foldedNotifications.size;
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

// ---------------------------------------------------------------------------
// The toast's lifetime: it lasts as long as the row is unread
// ---------------------------------------------------------------------------

/** The ids whose toasts a read has made stale: held as unread a moment ago,
 *  not any more.
 *
 *  A TRANSITION, never bare absence. A toast rides `PresenceCheck` while the
 *  reload `NotificationCreated` kicked off is still in flight, so a
 *  brand-new row reads as absent too. Dropping on absence would take down the
 *  toast for a notification nobody has seen yet. */
export function toastsToDrop(
  previous: readonly string[],
  current: readonly string[],
): string[] {
  const live = new Set(current);
  return previous.filter((id) => !live.has(id));
}

/** Take down one notification's toast.
 *
 *  Structural removal. The row was read, not deferred by the reader, so this
 *  must record no user dismissal: see `removeToast` vs `dismissToast`. */
export function dropNotificationToast(id: string): void {
  removeToast(notificationToastKey(id));
  unfoldFromOverflow(id);
}

/** Take down every notification toast, for a Mark all read. */
export function dropAllNotificationToasts(): void {
  for (const t of toasts.value) {
    if (t.key?.startsWith(NOTIFICATION_TOAST_PREFIX)) removeToast(t.key);
  }
  foldedNotifications.clear();
  removeToast(OVERFLOW_TOAST_KEY);
}

/** The unread set as it stood at the last sample, or null before the first. */
let lastUnreadIds: string[] | null = null;

/** Drop the toast of every notification that just left the unread set.
 *
 *  The bell badge, the Unread tab and the toast are three projections of one
 *  set. A row that is no longer unread can hold no surface. Reaching a tap
 *  target is the case that reported this: the *seen target* rule marks the row
 *  read and the badge falls, while the toast went on offering to open what was
 *  already on screen. See system-knowhow/notifications.md §4.
 *
 *  This is the page's OWN picture, so it answers on the tick the reader acts.
 *  A read it cannot see is the `NotificationRead` arm's to report, which is
 *  slower by a round trip and authoritative. */
function reconcileToastsWithUnread(set: Loadable<Notification[]>): void {
  // Not knowing the unread set is not the same as knowing it is empty. Hold the
  // baseline through a reconnect or a workspace switch, so neither clears a
  // live toast, and the read that follows is still a transition.
  if (set.status !== 'loaded') return;
  const current = set.data.map((n) => n.id);
  const previous = lastUnreadIds;
  lastUnreadIds = current;
  if (previous === null) return;
  for (const id of toastsToDrop(previous, current)) dropNotificationToast(id);
}

let lifetimeInstalled = false;

/** Subscribe the toasts to the unread set. Wired from `store/effects.ts`.
 *  The seen target watch (`shellStartup.ts`) is the loudest reason it exists.
 *
 *  Reading the set IS the subscription, and the work runs `untracked` because
 *  it writes `toasts` after reading it. Tracked, the first drop would make
 *  every toast in the app re-run this, and each drop would re-enter it. */
export function installNotificationToastLifetime(): void {
  if (lifetimeInstalled) return;
  lifetimeInstalled = true;
  effect(() => {
    const set = unreadNotifications.value;
    untracked(() => reconcileToastsWithUnread(set));
  });
}

/** Test-only: forget the baseline and the pile so one suite cannot leak into
 *  the next. The installed effect is a module-level singleton and stays. */
export function _resetToastLifetimeForTesting(): void {
  lastUnreadIds = null;
  foldedNotifications.clear();
}
