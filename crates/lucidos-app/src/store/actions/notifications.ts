import {
  notifications,
  showToast,
  unreadCount,
  notificationsFilter,
  notificationsHasMore,
  notificationsLoadingMore,
  notificationsModalOpen,
  notificationModalDetail,
  activeMenuItem,
} from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import { savePreference } from './preferences';
import {
  getNotifications,
  getNotification,
  markNotificationRead,
  markAllNotificationsRead,
} from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { createFailureCounter } from '../../utils/failureCounter';

const PAGE_SIZE = 15;

/** Load the first page of notifications using the current filter. */
export async function loadNotifications(): Promise<void> {
  setLoadingIfFresh(notifications);
  try {
    const data = await getNotifications({
      limit: PAGE_SIZE,
      filter: notificationsFilter.value,
    });
    notifications.value = { status: 'loaded', data: data.notifications || [] };
    unreadCount.value = data.unread_count;
    notificationsHasMore.value = data.has_more;
  } catch (error) {
    notifications.value = toFailed(error);
  }
}

/** Load the next page of notifications (infinite scroll). */
export async function loadMoreNotifications(): Promise<void> {
  if (notificationsLoadingMore.value || !notificationsHasMore.value) return;

  const current = notifications.value;
  if (current.status !== 'loaded' || current.data.length === 0) return;

  const lastItem = current.data[current.data.length - 1];
  const beforeTs = new Date(lastItem.created_at).getTime() / 1000;

  notificationsLoadingMore.value = true;
  try {
    const data = await getNotifications({
      limit: PAGE_SIZE,
      before: beforeTs,
      filter: notificationsFilter.value,
    });
    notifications.value = {
      status: 'loaded',
      data: [...current.data, ...(data.notifications || [])],
    };
    unreadCount.value = data.unread_count;
    notificationsHasMore.value = data.has_more;
  } catch (error) {
    showToast(`Failed to load more notifications: ${errorDetail(error)}`, 'error');
  } finally {
    notificationsLoadingMore.value = false;
  }
}

/** Switch between "all" and "unread" filter and reload. */
export function setNotificationsFilter(filter: 'all' | 'unread'): void {
  notificationsFilter.value = filter;
  // Both loaders self-report failures via Loadable failed / showToast; no
  // outer catch needed. `void` is the explicit fire-and-forget marker.
  void loadNotifications();
  void savePreference('notifications_filter', filter);
}

/** Threshold for the refresh-unread escalation toast. Three consecutive
 *  poll failures before we bother the user — a single transient failure
 *  shouldn't surface, but a sustained outage should so the user knows the
 *  badge is stale. Reset on the next success. */
const REFRESH_UNREAD_TOAST_THRESHOLD = 3;
const refreshUnreadFailures = createFailureCounter(REFRESH_UNREAD_TOAST_THRESHOLD, () => {
  showToast(
    `Unread count is stale — couldn't reach the engine after ${REFRESH_UNREAD_TOAST_THRESHOLD} tries`,
    'error',
    { key: 'refresh-unread-count' },
  );
});

/** Lightweight fetch: just refresh the unread count without loading items.
 *  Called by SSE handlers + page-resume — runs without user intent, so we
 *  swallow individual failures (best-effort telemetry, see
 *  `.claude/rules/frontend.md` § "Carve-out: best-effort telemetry") and
 *  escalate via a single toast only after `REFRESH_UNREAD_TOAST_THRESHOLD`
 *  consecutive failures. */
export async function refreshUnreadCount(): Promise<void> {
  try {
    const data = await getNotifications({ limit: 0 });
    unreadCount.value = data.unread_count;
    refreshUnreadFailures.recordSuccess();
  } catch {
    refreshUnreadFailures.recordFailure();
  }
}

/** Handle notification SSE events (NotificationCreated/Read/AllRead).
 *  Skips full reload when the detail modal is open — the user is navigating
 *  through the list and a reload with `filter: 'unread'` would remove the
 *  currently-viewed item, breaking prev/next navigation. */
export function handleNotificationSSE(): void {
  void refreshUnreadCount();
  if (activeMenuItem.value === 'notifications' && !notificationsModalOpen.value) {
    void loadNotifications();
  }
}

// Deduplication: prevent the same notification from being opened twice in quick
// succession (e.g. SW postMessage + URL-param cold start both fire for one tap).
let _lastViewedId: string | null = null;
let _lastViewedAt = 0;

/** Reset the dedup guard so the same notification can be reopened after closing. */
export function resetViewDedup(): void {
  _lastViewedId = null;
}

/** Mark a notification read on tap: optimistic local cache update + best-effort
 *  API call. Idempotent — safe to call on already-read rows. */
export function markReadOptimistic(id: string): void {
  const current = notifications.value;
  if (current.status === 'loaded') {
    const cached = current.data.find((n) => n.id === id);
    if (cached && !cached.read) {
      notifications.value = {
        status: 'loaded',
        data: current.data.map((n) => (n.id === id ? { ...n, read: true } : n)),
      };
      if (unreadCount.value > 0) unreadCount.value -= 1;
    }
  }
  markNotificationRead(id).catch(() => { /* row stays unread; user sees it next visit */ });
}

export async function viewNotification(id: string): Promise<void> {
  const now = Date.now();
  if (id === _lastViewedId && now - _lastViewedAt < 10_000) return;
  _lastViewedId = id;
  _lastViewedAt = now;

  try {
    const notification = await getNotification(id);
    if (notification) {
      notificationModalDetail.value = notification;
      notificationsModalOpen.value = true;
      markReadOptimistic(id);
    }
  } catch (error) {
    showToast('Failed to load notification: ' + errorDetail(error), 'error');
  }
}

/** Navigate the modal to another notification by id. Owns the signal writes
 *  the modal previously did inline. Returns the loaded id, or null on
 *  failure / unknown target. */
export async function navigateToNotification(targetId: string): Promise<string | null> {
  const list = notifications.value;
  const target = list.status === 'loaded'
    ? list.data.find((n) => n.id === targetId)
    : undefined;
  if (!target) return null;

  try {
    // Only POST the read flip when it would change something — clicking
    // through already-read entries is the common case and used to round-trip
    // for nothing.
    const reads = target.read
      ? [Promise.resolve(null), getNotification(target.id)] as const
      : [markNotificationRead(target.id), getNotification(target.id)] as const;
    const [, full] = await Promise.all(reads);
    if (!full) return null;

    notificationModalDetail.value = full;
    if (!target.read) {
      const after = notifications.value;
      if (after.status === 'loaded') {
        notifications.value = {
          status: 'loaded',
          data: after.data.map((n) => n.id === target.id ? { ...n, read: true } : n),
        };
      }
      if (unreadCount.value > 0) unreadCount.value -= 1;
    }
    return full.id;
  } catch (error) {
    showToast('Failed to load notification: ' + errorDetail(error), 'error');
    return null;
  }
}

/** Close the notifications detail modal and refresh the list. The view layer
 *  must not flip the modal signals directly — components express intent. */
export function closeNotificationsModal(): void {
  notificationsModalOpen.value = false;
  notificationModalDetail.value = null;
  resetViewDedup();
  void loadNotifications();
}

export async function markAllRead(): Promise<void> {
  try {
    await markAllNotificationsRead();

    // Optimistic update: mark all items read and zero out the count
    const current = notifications.value;
    if (current.status === 'loaded') {
      notifications.value = {
        status: 'loaded',
        data: current.data.map((n) => ({ ...n, read: true })),
      };
    }
    unreadCount.value = 0;
  } catch (error) {
    showToast('Failed to mark all as read: ' + errorDetail(error), 'error');
  }
}
