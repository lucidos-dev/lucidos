
import { confirmSnapshotRestore } from './restore-snapshot';
import { confirmConfigUndo } from './undo-config-change';
import { effect, untracked } from '@preact/signals';
import type { PresenceOutcome } from '@lucidos/event-stream';
import type { Loadable, Notification } from '../types';
import { markReadOptimistic, viewNotification } from './notifications';
import { switchMenuItem } from './menu';
import { handleNavigationRequest } from './navigation-dispatch';
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

export function notificationToastKey(id: string): string {
  return `${NOTIFICATION_TOAST_PREFIX}${id}`;
}

export function isShellActive(
  pageActive: boolean,
  nativeFullscreen: boolean,
  pseudoFullscreen: boolean,
): boolean {
  return pageActive && !nativeFullscreen && !pseudoFullscreen;
}

export function shellIsActive(): boolean {
  return isShellActive(
    isPageActive(),
    appFullscreenHost.value !== null,
    appPseudoFullscreen.value,
  );
}

export type InAppRow = 'row1_auto_read' | 'row2_or_3_toast_and_badge' | 'shell_not_active';

export function classifyInAppRow(target: DeepLinkTarget): InAppRow {
  if (!shellIsActive()) return 'shell_not_active';
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

export function dispatchDeepLink(target: DeepLinkTarget): boolean {
  const action = resolveDeepLink(target);
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
      if (action.notification) markReadOptimistic(action.notification, target.restoreGeneration);
      handleNavigationRequest(action.to);
      return dispatched();
    case 'restore-snapshot':
      if (action.notification) markReadOptimistic(action.notification, target.restoreGeneration);
      void confirmSnapshotRestore(action.tap);
      return dispatched();
    case 'undo-config-change':
      if (action.notification) markReadOptimistic(action.notification, target.restoreGeneration);
      void confirmConfigUndo(action.tap);
      return dispatched();
    case 'view-notification':
      void viewNotification(action.id, target.restoreGeneration);
      return dispatched();
    case 'noop':
      return false;
  }
}

function dispatched(): boolean {
  repaintLandedContent();
  return true;
}

export interface InAppNotification {
  title: string;
  body: string;
  target: DeepLinkTarget;
}

export function presentInAppNotification({ title, body, target }: InAppNotification): PresenceOutcome {
  if (workspaceUnavailable()) return 'not_shown';
  const row = classifyInAppRow(target);

  if (row === 'shell_not_active') return 'not_shown';

  if (row === 'row1_auto_read') {
    if (target.notification) markReadOptimistic(target.notification);
    return 'auto_read';
  }

  if (!currentNotificationToasts()) return 'silenced';

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

  let onClick: (() => void) | undefined;
  if (resolved.type !== 'noop') {
    let acted = false;
    onClick = () => {
      if (acted) return;
      acted = true;
      if (toastKey) dismissToast(toastKey);
      if (resolved.type === 'view-notification') {
        const id = resolved.id;
        const ensureRead = () => markReadOptimistic(id);
        void viewNotification(id).then(ensureRead, ensureRead);
      } else {
        dispatchDeepLink(target);
      }
    };
  }

  const [toastTitle, message] = body ? [safeTitle, body] : [undefined, safeTitle];
  showToast(message, 'info', { title: toastTitle, key: toastKey, onClick });
  return 'shown';
}

const foldedNotifications = new Set<string>();
let anonymousFolds = 0;

function overflowIsShowing(): boolean {
  return toasts.value.some((t) => t.key === OVERFLOW_TOAST_KEY);
}

function foldIntoOverflow(notifId: string | null): void {
  if (!overflowIsShowing()) foldedNotifications.clear();
  foldedNotifications.add(notifId ?? `anonymous-${++anonymousFolds}`);
  showOverflowToast();
}

function unfoldFromOverflow(id: string): void {
  if (!overflowIsShowing()) {
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


export function toastsToDrop(
  previous: readonly string[],
  current: readonly string[],
): string[] {
  const live = new Set(current);
  return previous.filter((id) => !live.has(id));
}

export function dropNotificationToast(id: string): void {
  removeToast(notificationToastKey(id));
  unfoldFromOverflow(id);
}

export function dropAllNotificationToasts(): void {
  for (const t of toasts.value) {
    if (t.key?.startsWith(NOTIFICATION_TOAST_PREFIX)) removeToast(t.key);
  }
  foldedNotifications.clear();
  removeToast(OVERFLOW_TOAST_KEY);
}

let lastUnreadIds: string[] | null = null;

function reconcileToastsWithUnread(set: Loadable<Notification[]>): void {
  if (set.status !== 'loaded') return;
  const current = set.data.map((n) => n.id);
  const previous = lastUnreadIds;
  lastUnreadIds = current;
  if (previous === null) return;
  for (const id of toastsToDrop(previous, current)) dropNotificationToast(id);
}

let lifetimeInstalled = false;

export function installNotificationToastLifetime(): void {
  if (lifetimeInstalled) return;
  lifetimeInstalled = true;
  effect(() => {
    const set = unreadNotifications.value;
    untracked(() => reconcileToastsWithUnread(set));
  });
}

export function _resetToastLifetimeForTesting(): void {
  lastUnreadIds = null;
  foldedNotifications.clear();
}
