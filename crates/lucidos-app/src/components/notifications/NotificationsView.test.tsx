import { describe, it, expect, vi } from 'vitest';
import type { Loadable, Notification } from '../../store/types';

// NotificationsView imports the actions module, which imports the API client.
// Mock it (no real HTTP) so importing the component's pure selector is
// side-effect-free — mirrors notifications.test.ts.
vi.mock('../../api/client', () => ({
  getNotifications: vi.fn().mockResolvedValue({ notifications: [], unread_count: 0, has_more: false }),
  getNotification: vi.fn(),
  markNotificationRead: vi.fn(),
  markAllNotificationsRead: vi.fn(),
  isTransportError: () => false,
}));

const { notificationsTabSource, notificationsTabIsLoading } = await import('./NotificationsView');
const { unreadNotifications, notifications, unreadCount } = await import('../../store/store');

function n(id: string, read: boolean): Notification {
  return { id, title: id, message: id, read, created_at: new Date().toISOString() };
}

describe('notificationsTabSource — single source of truth for the Unread tab', () => {
  const unread: Loadable<Notification[]> = { status: 'loaded', data: [n('u1', false), n('u2', false)] };
  const all: Loadable<Notification[]> = { status: 'loaded', data: [n('a1', true), n('u1', false)] };

  it('renders the unread set (the bell + app-icon badge source) on the "Unread" tab', () => {
    // Same reference the badge counts — not a second fetch that could drift.
    expect(notificationsTabSource('unread', unread, all)).toBe(unread);
  });

  it('renders the paginated browse list on the "All" tab', () => {
    expect(notificationsTabSource('all', unread, all)).toBe(all);
  });

  it("the Unread tab's item count IS the bell-badge count (cannot disagree)", () => {
    // Wire the live signals the way the app does: the badge derives from
    // `unreadNotifications`, and the Unread tab sources the same signal. So the
    // number of rows the Unread tab renders always equals `unreadCount` — the
    // exact invariant the reported bug violated.
    unreadNotifications.value = { status: 'loaded', data: [n('u1', false), n('u2', false), n('u3', false)] };
    notifications.value = { status: 'loaded', data: [n('a1', true)] }; // browse list is unrelated

    const tab = notificationsTabSource('unread', unreadNotifications.value, notifications.value);
    const rendered = tab.status === 'loaded' ? tab.data.length : 0;

    expect(rendered).toBe(unreadCount.value);
    expect(rendered).toBe(3);
  });
});

describe('notificationsTabIsLoading: what the panel owes a skeleton', () => {
  it("counts 'not-loaded' as loading, on either tab", () => {
    // The reported blank panel. Between the panel opening and its loader
    // claiming the signal, the tab holds 'not-loaded' and no rows. Reading only
    // 'loading' left that window drawing nothing at all.
    expect(notificationsTabIsLoading({ status: 'not-loaded' })).toBe(true);
  });

  it("counts 'loading' as loading", () => {
    expect(notificationsTabIsLoading({ status: 'loading' })).toBe(true);
  });

  it('stops once rows land, empty ones included', () => {
    // An empty inbox is an answer, and it gets the empty state, never a
    // skeleton that would claim rows are still on their way.
    expect(notificationsTabIsLoading({ status: 'loaded', data: [] })).toBe(false);
    expect(notificationsTabIsLoading({ status: 'loaded', data: [n('a1', true)] })).toBe(false);
  });

  it('stops on failure, which the error state owns instead', () => {
    expect(notificationsTabIsLoading({ status: 'failed', error: 'nope' })).toBe(false);
  });
});
