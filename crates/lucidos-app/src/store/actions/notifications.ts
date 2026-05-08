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
  loadNotifications();
  savePreference('notifications_filter', filter);
}

/** Lightweight fetch: just refresh the unread count without loading items. */
export async function refreshUnreadCount(): Promise<void> {
  try {
    const data = await getNotifications({ limit: 0 });
    unreadCount.value = data.unread_count;
  } catch {
    // Polling failure — silently ignore; next poll will retry
  }
}

/** Handle notification SSE events (NotificationCreated/Read/AllRead).
 *  Skips full reload when the detail modal is open — the user is navigating
 *  through the list and a reload with `filter: 'unread'` would remove the
 *  currently-viewed item, breaking prev/next navigation. */
export function handleNotificationSSE(): void {
  refreshUnreadCount();
  if (activeMenuItem.value === 'notifications' && !notificationsModalOpen.value) {
    loadNotifications();
  }
}

// Deduplication: prevent the same notification from being opened twice in quick
// succession (e.g. both notification-clicked and notification-pushed resolve).
let _lastViewedId: string | null = null;
let _lastViewedAt = 0;

/** Reset the dedup guard so the same notification can be reopened after closing. */
export function resetViewDedup(): void {
  _lastViewedId = null;
}

export async function viewNotification(id: string): Promise<void> {
  const now = Date.now();
  if (id === _lastViewedId && now - _lastViewedAt < 10_000) return;
  _lastViewedId = id;
  _lastViewedAt = now;

  try {
    const [, notification] = await Promise.all([
      markNotificationRead(id),
      getNotification(id),
    ]);
    if (notification) {
      notificationModalDetail.value = notification;
      notificationsModalOpen.value = true;

      // Read pre-mark state from the cached list — the parallel getNotification
      // fetch may already reflect the post-mark state, racing with markNotificationRead.
      const current = notifications.value;
      let wasUnread = !notification.read;
      if (current.status === 'loaded') {
        const cached = current.data.find((n) => n.id === id);
        if (cached) wasUnread = !cached.read;
        notifications.value = {
          status: 'loaded',
          data: current.data.map((n) =>
            n.id === id ? { ...n, read: true } : n
          ),
        };
      }
      if (wasUnread && unreadCount.value > 0) {
        unreadCount.value = unreadCount.value - 1;
      }
    }
  } catch (error) {
    showToast('Failed to load notification: ' + errorDetail(error), 'error');
  }
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
