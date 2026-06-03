import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  notifications,
  notificationsFilter,
  notificationsModalOpen,
  notificationModalDetail,
  activeMenuItem,
  toasts,
  unreadCount,
} from '../store';
import type { Notification } from '../types';

// Mock the API client to prevent real HTTP calls
vi.mock('../../api/client', () => ({
  getNotifications: vi.fn().mockResolvedValue({ notifications: [], unread_count: 0, has_more: false }),
  getNotification: vi.fn(),
  markNotificationRead: vi.fn(),
  markAllNotificationsRead: vi.fn(),
}));

const { handleNotificationSSE, refreshUnreadCount } = await import('./notifications');
const { getNotifications } = await import('../../api/client');

function makeNotification(id: string, read: boolean): Notification {
  return {
    id,
    title: `Notification ${id}`,
    message: `Message ${id}`,
    read,
    created_at: new Date().toISOString(),
  };
}

describe('handleNotificationSSE', () => {
  beforeEach(() => {
    activeMenuItem.value = 'notifications';
    notificationsFilter.value = 'unread';
    notificationsModalOpen.value = false;
    notificationModalDetail.value = null;
    notifications.value = {
      status: 'loaded',
      data: [
        makeNotification('a', false),
        makeNotification('b', false),
        makeNotification('c', false),
      ],
    };
  });

  it('does NOT reload notifications when modal is open', async () => {
    // User is viewing a notification in the modal
    notificationsModalOpen.value = true;
    notificationModalDetail.value = makeNotification('a', true);

    // SSE event arrives (e.g. NotificationRead)
    handleNotificationSSE();

    // The list should NOT be reloaded — items must stay intact for navigation
    expect(notifications.value.status).toBe('loaded');
    if (notifications.value.status === 'loaded') {
      expect(notifications.value.data).toHaveLength(3);
      expect(notifications.value.data.map((n) => n.id)).toEqual(['a', 'b', 'c']);
    }
  });

  it('reloads notifications when modal is closed and panel is active', () => {
    notificationsModalOpen.value = false;
    (getNotifications as ReturnType<typeof vi.fn>).mockClear();

    handleNotificationSSE();

    // Existing data stays visible through the refetch round-trip.
    expect(notifications.value.status).toBe('loaded');
    expect(getNotifications).toHaveBeenCalled();
  });

  it('does NOT reload when notifications panel is not active', () => {
    activeMenuItem.value = 'files';

    handleNotificationSSE();

    // Should not reload — still 'loaded' with original data
    expect(notifications.value.status).toBe('loaded');
  });
});

describe('refreshUnreadCount failure handling', () => {
  beforeEach(async () => {
    toasts.value = [];
    unreadCount.value = 0;
    // Reset the module-level failure counter — every test starts with a
    // successful refresh so failures are independent across tests.
    (getNotifications as ReturnType<typeof vi.fn>).mockReset();
    (getNotifications as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      notifications: [], unread_count: 0, has_more: false,
    });
    await refreshUnreadCount();
    toasts.value = [];
  });

  it('does not toast on a single transient failure (best-effort poll)', async () => {
    (getNotifications as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('boom'));

    await refreshUnreadCount();

    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(0);
  });

  it('toasts after THREE consecutive failures and then stays quiet', async () => {
    (getNotifications as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('boom'));

    await refreshUnreadCount();
    await refreshUnreadCount();
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(0);

    await refreshUnreadCount();
    const errors = toasts.value.filter((t) => t.type === 'error');
    expect(errors).toHaveLength(1);
    expect(errors[0].message).toMatch(/unread count is stale/i);

    // 4th, 5th failures don't re-toast — same outage, one notice.
    await refreshUnreadCount();
    await refreshUnreadCount();
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(1);
  });

  it('resets the failure counter on a successful refresh', async () => {
    (getNotifications as ReturnType<typeof vi.fn>)
      .mockRejectedValueOnce(new Error('boom'))
      .mockRejectedValueOnce(new Error('boom'))
      .mockResolvedValueOnce({ notifications: [], unread_count: 7, has_more: false })
      .mockRejectedValue(new Error('boom'));

    await refreshUnreadCount();
    await refreshUnreadCount();
    await refreshUnreadCount(); // success — counter resets
    expect(unreadCount.value).toBe(7);

    // Two more failures shouldn't trip the threshold yet (we're back to 0).
    await refreshUnreadCount();
    await refreshUnreadCount();
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(0);
  });
});
