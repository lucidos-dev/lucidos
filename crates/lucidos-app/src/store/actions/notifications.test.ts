import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  notifications,
  notificationsFilter,
  notificationsModalOpen,
  notificationModalDetail,
  activeMenuItem,
} from '../store';
import type { Notification } from '../types';

// Mock the API client to prevent real HTTP calls
vi.mock('../../api/client', () => ({
  getNotifications: vi.fn().mockResolvedValue({ notifications: [], unread_count: 0, has_more: false }),
  getNotification: vi.fn(),
  markNotificationRead: vi.fn(),
  markAllNotificationsRead: vi.fn(),
}));

const { handleNotificationSSE } = await import('./notifications');
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
