import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  notifications,
  unreadNotifications,
  unreadCount,
  notificationsFilter,
  notificationsHasMore,
  notificationsLoadingMore,
  panelOverlay,
  notificationDetailPending,
  activeMenuItem,
  toasts,
  connectionStatus,
  focusedPane,
  servedNotifications,
  servedUnreadNotifications,
  pendingNotificationReads,
  servedNotificationEpochs,
} from '../store';
import type { Notification, Loadable } from '../types';

vi.mock('../../api/client', async () => {
  const core = await vi.importActual<typeof import('../../api/client/_core')>('../../api/client/_core');
  return {
    getNotifications: vi.fn().mockResolvedValue({ notifications: [], unread_count: 0, has_more: false }),
    getNotification: vi.fn(),
    markNotificationRead: vi.fn().mockResolvedValue({ success: true }),
    markAllNotificationsRead: vi.fn(),
    isTransportError: core.isTransportError,
    retryTransient: core.retryTransient,
  };
});

vi.mock('./pane', () => ({ revealContentPane: vi.fn() }));
vi.mock('./navigation', () => ({ pushNavState: vi.fn(), replaceNavState: vi.fn() }));

vi.mock('../../utils/platform', async (importActual) => ({
  ...(await importActual<typeof import('../../utils/platform')>()),
  isTauri: vi.fn(() => false),
}));
vi.mock('../../utils/tauri', async (importActual) => ({
  ...(await importActual<typeof import('../../utils/tauri')>()),
  nudgeDockBadge: vi.fn(),
}));
vi.mock('./app-badge', () => ({
  syncWorkspaceAppBadge: vi.fn(),
  applyAppBadge: vi.fn(),
}));

const {
  refreshActiveNotificationsTab,
  handleNotificationSSE,
  loadUnreadNotifications,
  markAllRead,
  markReadOptimistic,
  loadNotifications,
  loadMoreNotifications,
  navigateAdjacentNotification,
  stepViewedNotification,
  viewNotification,
  resetViewDedup,
  _resetNotificationLoadsForTesting,
  unreadNotificationsSlice,
} = await import('./notifications');
const { getNotifications, getNotification, markNotificationRead, markAllNotificationsRead } = await import('../../api/client');
const { revealContentPane } = await import('./pane');
const { pushNavState } = await import('./navigation');
const { isTauri } = await import('../../utils/platform');
const { nudgeDockBadge } = await import('../../utils/tauri');
const { syncWorkspaceAppBadge } = await import('./app-badge');

type Mock = ReturnType<typeof vi.fn>;
type NotifResponse = { notifications: Notification[]; unread_count: number; has_more: boolean };

function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void } {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}

function makeNotification(id: string, read: boolean): Notification {
  return {
    id,
    title: `Notification ${id}`,
    message: `Message ${id}`,
    read,
    created_at: new Date().toISOString(),
  };
}

function makeUnread(n: number): Notification[] {
  return Array.from({ length: n }, (_, i) => makeNotification(`u${i}`, false));
}

function loadedRows(list: Loadable<Notification[]>): Notification[] {
  if (list.status !== 'loaded') throw new Error(`list is ${list.status}`);
  return list.data;
}

beforeEach(() => {
  _resetNotificationLoadsForTesting();
  pendingNotificationReads.value = new Map();
  servedNotificationEpochs.value = { browse: 0, unread: 0 };
});

function seedUnread(items: Notification[]): void {
  servedUnreadNotifications.value = { status: 'loaded', data: items };
}

describe('handleNotificationSSE', () => {
  beforeEach(() => {
    activeMenuItem.value = 'notifications';
    notificationsFilter.value = 'all';
    panelOverlay.value = null;
    seedUnread([]);
    servedNotifications.value = {
      status: 'loaded',
      data: [
        makeNotification('a', false),
        makeNotification('b', false),
        makeNotification('c', false),
      ],
    };
    (getNotifications as Mock).mockReset();
    (getNotifications as Mock).mockResolvedValue({ notifications: [], unread_count: 0, has_more: false });
  });

  it('ALWAYS reloads the unread set (bell + app-icon badge AND the Unread tab, one source)', () => {
    handleNotificationSSE();
    expect(getNotifications).toHaveBeenCalledWith(expect.objectContaining({ filter: 'unread' }));
  });

  it('reloads the "All" browse list when it is the visible tab with no detail open', () => {
    handleNotificationSSE();

    expect(getNotifications).toHaveBeenCalledWith(expect.objectContaining({ filter: 'all' }));
    expect(notifications.value.status).toBe('loaded');
  });

  it('does NOT reload the "All" browse list when a notification detail is open', () => {
    panelOverlay.value = { type: 'notification-detail', notification: makeNotification('a', true) };

    handleNotificationSSE();

    expect(getNotifications).toHaveBeenCalledWith(expect.objectContaining({ filter: 'unread' }));
    expect(getNotifications).not.toHaveBeenCalledWith(expect.objectContaining({ filter: 'all' }));
    expect(notifications.value.status).toBe('loaded');
    if (notifications.value.status === 'loaded') {
      expect(notifications.value.data.map((n) => n.id)).toEqual(['a', 'b', 'c']);
    }
  });

  it('does NOT reload the "All" browse list when the notifications panel is not active', () => {
    activeMenuItem.value = 'files';

    handleNotificationSSE();

    expect(getNotifications).toHaveBeenCalledWith(expect.objectContaining({ filter: 'unread' }));
    expect(getNotifications).not.toHaveBeenCalledWith(expect.objectContaining({ filter: 'all' }));
    expect(notifications.value.status).toBe('loaded');
  });

  it('on the "Unread" tab refreshes ONLY the unread set — never the browse list', () => {
    notificationsFilter.value = 'unread';

    handleNotificationSSE();

    expect(getNotifications).toHaveBeenCalledWith(expect.objectContaining({ filter: 'unread' }));
    expect(getNotifications).not.toHaveBeenCalledWith(expect.objectContaining({ filter: 'all' }));
  });

  it('nudges the native dock badge under Tauri (instant desktop badge update)', () => {
    (isTauri as Mock).mockReturnValue(true);
    (nudgeDockBadge as Mock).mockClear();

    handleNotificationSSE();

    expect(nudgeDockBadge).toHaveBeenCalledTimes(1);
  });

  it('does NOT nudge the dock badge off Tauri (browser / PWA)', () => {
    (isTauri as Mock).mockReturnValue(false);
    (nudgeDockBadge as Mock).mockClear();

    handleNotificationSSE();

    expect(nudgeDockBadge).not.toHaveBeenCalled();
  });
});

describe('navigateAdjacentNotification', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    notificationsHasMore.value = false;
    notificationsLoadingMore.value = false;
    notificationsFilter.value = 'all';
    seedUnread([]);
    (getNotifications as Mock).mockReset();
    (getNotification as Mock).mockReset();
    (markNotificationRead as Mock).mockReset();
    (markNotificationRead as Mock).mockResolvedValue({ success: true });
  });

  it('steps within the loaded page from memory — no page fetch AND no detail GET', async () => {
    servedNotifications.value = {
      status: 'loaded',
      data: [makeNotification('a', true), makeNotification('b', true), makeNotification('c', true)],
    };

    const id = await navigateAdjacentNotification('a', 1);

    expect(id).toBe('b');
    expect(getNotifications).not.toHaveBeenCalled();
    expect(getNotification).not.toHaveBeenCalled();
    expect(panelOverlay.value).toMatchObject({
      type: 'notification-detail',
      notification: { id: 'b', message: 'Message b' },
    });
  });

  it('marks an unread stepped-to row read in the background without a detail GET', async () => {
    servedNotifications.value = {
      status: 'loaded',
      data: [makeNotification('a', true), makeNotification('b', false)],
    };

    const id = await navigateAdjacentNotification('a', 1);

    expect(id).toBe('b');
    expect(getNotification).not.toHaveBeenCalled();
    expect(markNotificationRead).toHaveBeenCalledWith('b');
    if (notifications.value.status === 'loaded') {
      expect(notifications.value.data.find((n) => n.id === 'b')?.read).toBe(true);
    }
  });

  it('does NOT POST a read flip when stepping to an already-read row', async () => {
    servedNotifications.value = {
      status: 'loaded',
      data: [makeNotification('a', true), makeNotification('b', true)],
    };

    await navigateAdjacentNotification('a', 1);

    expect(markNotificationRead).not.toHaveBeenCalled();
  });

  it('loads the next page when stepping older past the loaded boundary, then steps into it', async () => {
    servedNotifications.value = {
      status: 'loaded',
      data: [makeNotification('a', true), makeNotification('b', true)],
    };
    notificationsHasMore.value = true;
    (getNotifications as Mock).mockResolvedValueOnce({
      notifications: [makeNotification('c', true), makeNotification('d', true)],
      unread_count: 0,
      has_more: false,
    });

    const id = await navigateAdjacentNotification('b', 1);

    expect(getNotifications).toHaveBeenCalledTimes(1);
    expect(getNotification).not.toHaveBeenCalled();
    expect(id).toBe('c');
    if (notifications.value.status === 'loaded') {
      expect(notifications.value.data.map((n) => n.id)).toEqual(['a', 'b', 'c', 'd']);
    }
  });

  it('does not step past the last loaded item when the server has no more pages', async () => {
    servedNotifications.value = {
      status: 'loaded',
      data: [makeNotification('a', true), makeNotification('b', true)],
    };
    notificationsHasMore.value = false;

    const id = await navigateAdjacentNotification('b', 1);

    expect(id).toBeNull();
    expect(getNotifications).not.toHaveBeenCalled();
  });

  it('never fetches a page in the newer (prev) direction — the newest is always loaded', async () => {
    servedNotifications.value = {
      status: 'loaded',
      data: [makeNotification('a', true), makeNotification('b', true)],
    };
    notificationsHasMore.value = true;

    const id = await navigateAdjacentNotification('a', -1);

    expect(id).toBeNull();
    expect(getNotifications).not.toHaveBeenCalled();
  });
});

describe('stepViewedNotification', () => {
  beforeEach(() => {
    notificationsHasMore.value = false;
    servedNotifications.value = {
      status: 'loaded',
      data: [makeNotification('a', true), makeNotification('b', true)],
    };
    panelOverlay.value = { type: 'notification-detail', notification: makeNotification('a', true) };
    focusedPane.value = 'content';
  });

  it('steps older from the open notification when the content pane is focused', async () => {
    expect(stepViewedNotification(1)).toBe(true);
    await Promise.resolve();
    expect(panelOverlay.value).toMatchObject({ notification: { id: 'b' } });
  });

  it('claims the key at the end of the list, so it never falls through to the thread', () => {
    expect(stepViewedNotification(-1)).toBe(true);
    expect(panelOverlay.value).toMatchObject({ notification: { id: 'a' } });
  });

  it('declines when another pane is focused', () => {
    focusedPane.value = 'thread';
    expect(stepViewedNotification(1)).toBe(false);
    expect(panelOverlay.value).toMatchObject({ notification: { id: 'a' } });
  });

  it('declines when no notification is open', () => {
    panelOverlay.value = null;
    expect(stepViewedNotification(1)).toBe(false);
  });
});

describe('viewNotification loads the inbox list so detail chevrons work', () => {
  beforeEach(() => {
    resetViewDedup();
    panelOverlay.value = null;
    notificationsFilter.value = 'all';
    seedUnread([]);
    (getNotifications as Mock).mockReset();
    (getNotification as Mock).mockReset();
    (markNotificationRead as Mock).mockReset();
    (markNotificationRead as Mock).mockResolvedValue({ success: true });
  });

  it('loads the browse list on the deep-link path (list not yet loaded)', async () => {
    servedNotifications.value = { status: 'not-loaded' };
    (getNotification as Mock).mockResolvedValueOnce(makeNotification('x', false));
    (getNotifications as Mock).mockResolvedValueOnce({
      notifications: [makeNotification('x', false), makeNotification('y', false)],
      unread_count: 2,
      has_more: false,
    });

    await viewNotification('x');

    expect(panelOverlay.value).toMatchObject({
      type: 'notification-detail',
      notification: { id: 'x' },
    });
    expect(getNotifications).toHaveBeenCalledTimes(1);
    const list = notifications.value as Loadable<Notification[]>;
    if (list.status === 'loaded') {
      expect(list.data.some((n) => n.id === 'x')).toBe(true);
    } else {
      throw new Error('expected the browse list to be loaded');
    }
    expect(markNotificationRead).toHaveBeenCalledWith('x');
  });

  it('does NOT reload when the browse list already holds the row (in-app open)', async () => {
    servedNotifications.value = {
      status: 'loaded',
      data: [makeNotification('x', false), makeNotification('y', false)],
    };

    await viewNotification('x');

    expect(panelOverlay.value).toMatchObject({ type: 'notification-detail', notification: { id: 'x' } });
    expect(getNotifications).not.toHaveBeenCalled();
  });
});

describe('viewNotification opens from memory, without a detail GET', () => {
  beforeEach(() => {
    resetViewDedup();
    panelOverlay.value = null;
    notificationsFilter.value = 'all';
    servedNotifications.value = { status: 'not-loaded' };
    seedUnread([]);
    (getNotifications as Mock).mockReset();
    (getNotifications as Mock).mockResolvedValue({ notifications: [], unread_count: 0, has_more: false });
    (getNotification as Mock).mockReset();
    (markNotificationRead as Mock).mockReset();
    (markNotificationRead as Mock).mockResolvedValue({ success: true });
    (revealContentPane as Mock).mockClear();
    (pushNavState as Mock).mockClear();
    notificationDetailPending.value = null;
    toasts.value = [];
  });

  it('opens SYNCHRONOUSLY from the browse list, with no await and no fetch', () => {
    servedNotifications.value = {
      status: 'loaded',
      data: [makeNotification('x', false), makeNotification('y', false)],
    };

    void viewNotification('x');

    expect(panelOverlay.value).toMatchObject({ type: 'notification-detail', notification: { id: 'x' } });
    expect(getNotification).not.toHaveBeenCalled();
  });

  it('opens SYNCHRONOUSLY from the unread set (the "Unread" tab renders that list)', () => {
    notificationsFilter.value = 'unread';
    seedUnread([makeNotification('u0', false), makeNotification('u1', false)]);

    void viewNotification('u0');

    expect(panelOverlay.value).toMatchObject({ type: 'notification-detail', notification: { id: 'u0' } });
    expect(getNotification).not.toHaveBeenCalled();
  });

  it('reveals the content pane and pushes a nav entry on the memory path too', () => {
    servedNotifications.value = { status: 'loaded', data: [makeNotification('x', false)] };

    void viewNotification('x');

    expect(revealContentPane).toHaveBeenCalled();
    expect(pushNavState).toHaveBeenCalled();
  });

  it('still marks the row read on the memory path', async () => {
    servedNotifications.value = { status: 'loaded', data: [makeNotification('x', false)] };

    await viewNotification('x');

    expect(markNotificationRead).toHaveBeenCalledWith('x');
  });

  it('falls back to the detail GET when NEITHER list holds the row (cold push tap)', async () => {
    servedNotifications.value = { status: 'not-loaded' };
    servedUnreadNotifications.value = { status: 'not-loaded' };
    (getNotification as Mock).mockResolvedValueOnce(makeNotification('x', false));

    await viewNotification('x');

    expect(getNotification).toHaveBeenCalledWith('x');
    expect(panelOverlay.value).toMatchObject({ type: 'notification-detail', notification: { id: 'x' } });
  });

  it('falls back to the detail GET when the lists are loaded but lack the row', async () => {
    servedNotifications.value = { status: 'loaded', data: [makeNotification('other', true)] };
    seedUnread([]);
    (getNotification as Mock).mockResolvedValueOnce(makeNotification('x', false));

    await viewNotification('x');

    expect(getNotification).toHaveBeenCalledWith('x');
    expect(panelOverlay.value).toMatchObject({ type: 'notification-detail', notification: { id: 'x' } });
  });

  it('flags the pending fetch and reveals the pane on the miss path, then clears it', async () => {
    servedNotifications.value = { status: 'not-loaded' };
    servedUnreadNotifications.value = { status: 'not-loaded' };
    const gate = deferred<Notification>();
    (getNotification as Mock).mockReturnValueOnce(gate.promise);

    const pending = viewNotification('x');
    expect(notificationDetailPending.value).toBe('x');
    expect(revealContentPane).toHaveBeenCalled();
    expect(panelOverlay.value).toBeNull();

    gate.resolve(makeNotification('x', false));
    await pending;

    expect(notificationDetailPending.value).toBeNull();
    expect(panelOverlay.value).toMatchObject({ type: 'notification-detail', notification: { id: 'x' } });
  });

  it('clears the pending flag and pushes no nav entry when the fetch fails', async () => {
    servedNotifications.value = { status: 'not-loaded' };
    servedUnreadNotifications.value = { status: 'not-loaded' };
    (getNotification as Mock).mockRejectedValueOnce(new Error('engine down'));

    await viewNotification('x');

    expect(notificationDetailPending.value).toBeNull();
    expect(panelOverlay.value).toBeNull();
    expect(pushNavState).not.toHaveBeenCalled();
    expect(toasts.value.some((t) => t.message.startsWith('Failed to load notification'))).toBe(true);
  });

  it('never flags a pending fetch on the memory path', async () => {
    servedNotifications.value = { status: 'loaded', data: [makeNotification('x', false)] };

    await viewNotification('x');

    expect(notificationDetailPending.value).toBeNull();
  });

  it('loads the browse list for the chevrons when only the unread set held the row', async () => {
    notificationsFilter.value = 'unread';
    servedNotifications.value = { status: 'not-loaded' };
    seedUnread([makeNotification('u0', false)]);
    (getNotifications as Mock).mockResolvedValueOnce({
      notifications: [makeNotification('u0', false), makeNotification('u1', false)],
      unread_count: 2,
      has_more: false,
    });

    const pending = viewNotification('u0');
    expect(panelOverlay.value).toMatchObject({ type: 'notification-detail', notification: { id: 'u0' } });
    await pending;

    const list = notifications.value as Loadable<Notification[]>;
    if (list.status !== 'loaded') throw new Error('expected the browse list to be loaded');
    expect(list.data.some((n) => n.id === 'u0')).toBe(true);
  });
});

describe('the bell badge is derived from the unread set', () => {
  beforeEach(() => {
    seedUnread([]);
    servedNotifications.value = { status: 'not-loaded' };
    (getNotifications as Mock).mockReset();
    (markNotificationRead as Mock).mockReset();
    (markNotificationRead as Mock).mockResolvedValue({ success: true });
  });

  it('reports the number of unread notifications in state', () => {
    seedUnread(makeUnread(3));
    expect(unreadCount.value).toBe(3);
    seedUnread([]);
    expect(unreadCount.value).toBe(0);
    servedUnreadNotifications.value = { status: 'not-loaded' };
    expect(unreadCount.value).toBe(0);
  });

  it('drops the badge when a notification is marked read, without a refetch', () => {
    seedUnread([makeNotification('x', false), makeNotification('y', false)]);
    expect(unreadCount.value).toBe(2);

    markReadOptimistic('x');

    expect(unreadCount.value).toBe(1);
    if (unreadNotifications.value.status === 'loaded') {
      expect(unreadNotifications.value.data.map((n) => n.id)).toEqual(['y']);
    }
    expect(getNotifications).not.toHaveBeenCalled();
  });

  it('loadNotifications (inbox browse) never touches the badge', async () => {
    seedUnread(makeUnread(2));
    (getNotifications as Mock).mockResolvedValueOnce({
      notifications: [makeNotification('a', true), makeNotification('b', true)],
      unread_count: 999,
      has_more: false,
    });

    await loadNotifications();

    expect(unreadCount.value).toBe(2);
    expect(notifications.value.status).toBe('loaded');
  });
});

describe('the app-icon badge is re-asserted, not diffed', () => {
  beforeEach(() => {
    seedUnread([]);
    servedNotifications.value = { status: 'not-loaded' };
    (getNotifications as Mock).mockReset();
    (getNotifications as Mock).mockResolvedValue({ notifications: [], unread_count: 0, has_more: false });
    (markNotificationRead as Mock).mockReset();
    (markNotificationRead as Mock).mockResolvedValue({ success: true });
    (markAllNotificationsRead as Mock).mockReset();
    (markAllNotificationsRead as Mock).mockResolvedValue({ success: true });
    (syncWorkspaceAppBadge as Mock).mockClear();
  });

  it('a reload landing the SAME count still re-asserts (the resume case)', async () => {
    await loadUnreadNotifications();

    expect(unreadCount.value).toBe(0);
    expect(syncWorkspaceAppBadge).toHaveBeenCalled();
  });

  it('marking read re-asserts even when the row was not in the unread set', () => {
    markReadOptimistic('never-loaded-here');

    expect(unreadCount.value).toBe(0);
    expect(syncWorkspaceAppBadge).toHaveBeenCalled();
  });

  it('markAllRead re-asserts', async () => {
    seedUnread(makeUnread(3));
    (syncWorkspaceAppBadge as Mock).mockClear();

    await markAllRead();

    expect(unreadCount.value).toBe(0);
    expect(syncWorkspaceAppBadge).toHaveBeenCalled();
  });

  it('a superseded load never writes the badge', async () => {
    const stale = deferred<NotifResponse>();
    (getNotifications as Mock)
      .mockReturnValueOnce(stale.promise)
      .mockResolvedValueOnce({ notifications: [], unread_count: 0, has_more: false });

    const first = loadUnreadNotifications();
    const second = loadUnreadNotifications();
    await second;
    (syncWorkspaceAppBadge as Mock).mockClear();

    stale.resolve({ notifications: makeUnread(4), unread_count: 4, has_more: false });
    await first;

    expect(unreadCount.value).toBe(0);
    expect(syncWorkspaceAppBadge).not.toHaveBeenCalled();
  });
});

describe('loadUnreadNotifications failure handling', () => {
  beforeEach(async () => {
    toasts.value = [];
    seedUnread([]);
    connectionStatus.value = 'connected';
    (getNotifications as Mock).mockReset();
    (getNotifications as Mock).mockResolvedValueOnce({
      notifications: [], unread_count: 0, has_more: false,
    });
    await loadUnreadNotifications();
    toasts.value = [];
  });

  afterEach(() => {
    connectionStatus.value = 'connecting';
  });

  it('does not toast on a single transient failure (best-effort poll)', async () => {
    (getNotifications as Mock).mockRejectedValueOnce(new Error('boom'));

    await loadUnreadNotifications();

    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(0);
  });

  it('toasts after THREE consecutive failures and then stays quiet', async () => {
    (getNotifications as Mock).mockRejectedValue(new Error('boom'));

    await loadUnreadNotifications();
    await loadUnreadNotifications();
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(0);

    await loadUnreadNotifications();
    const errors = toasts.value.filter((t) => t.type === 'error');
    expect(errors).toHaveLength(1);
    expect(errors[0].message).toMatch(/unread count is stale/i);

    await loadUnreadNotifications();
    await loadUnreadNotifications();
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(1);
  });

  it('does not count a browser-cancelled AbortError toward the threshold', async () => {
    (getNotifications as Mock).mockRejectedValue(new DOMException('aborted', 'AbortError'));

    await loadUnreadNotifications();
    await loadUnreadNotifications();
    await loadUnreadNotifications();
    await loadUnreadNotifications();
    await loadUnreadNotifications();

    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(0);
  });

  it('does not count a transport-layer TypeError ("Load failed") toward the threshold', async () => {
    (getNotifications as Mock).mockRejectedValue(new TypeError('Load failed'));

    await loadUnreadNotifications();
    await loadUnreadNotifications();
    await loadUnreadNotifications();
    await loadUnreadNotifications();
    await loadUnreadNotifications();

    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(0);
  });

  it('does not count ANY failure while the engine is unreachable', async () => {
    connectionStatus.value = 'disconnected';
    (getNotifications as Mock).mockRejectedValue(
      new DOMException('Request timed out after 10000ms', 'TimeoutError'),
    );

    for (let i = 0; i < 5; i++) await loadUnreadNotifications();

    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(0);
  });

  it('still toasts when the engine is REACHABLE but this endpoint keeps failing', async () => {
    connectionStatus.value = 'connected';
    (getNotifications as Mock).mockRejectedValue(
      new DOMException('Request timed out after 10000ms', 'TimeoutError'),
    );

    await loadUnreadNotifications();
    await loadUnreadNotifications();
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(0);

    await loadUnreadNotifications();
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(1);
  });

  it('retracts the stale-count card once a load lands', async () => {
    (getNotifications as Mock).mockRejectedValue(new Error('boom'));
    for (let i = 0; i < 3; i++) await loadUnreadNotifications();
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(1);

    (getNotifications as Mock).mockResolvedValue({
      notifications: [], unread_count: 0, has_more: false,
    });
    await loadUnreadNotifications();

    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(0);
  });

  it('resets the failure counter on a successful load', async () => {
    (getNotifications as Mock)
      .mockRejectedValueOnce(new Error('boom'))
      .mockRejectedValueOnce(new Error('boom'))
      .mockResolvedValueOnce({ notifications: makeUnread(7), unread_count: 7, has_more: false })
      .mockRejectedValue(new Error('boom'));

    await loadUnreadNotifications();
    await loadUnreadNotifications();
    await loadUnreadNotifications();
    expect(unreadCount.value).toBe(7);

    await loadUnreadNotifications();
    await loadUnreadNotifications();
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(0);
  });
});

describe('unread set is resilient to out-of-order responses', () => {
  beforeEach(() => {
    seedUnread([]);
    (getNotifications as Mock).mockReset();
    (markAllNotificationsRead as Mock).mockReset();
  });

  it('discards a stale created-reload that resolves after the read-reload', async () => {
    const created = deferred<NotifResponse>();
    (getNotifications as Mock)
      .mockReturnValueOnce(created.promise)
      .mockResolvedValueOnce({ notifications: [], unread_count: 0, has_more: false });

    const createdReload = loadUnreadNotifications();
    const readReload = loadUnreadNotifications();
    await readReload;
    expect(unreadCount.value).toBe(0);

    created.resolve({ notifications: [makeNotification('x', false)], unread_count: 1, has_more: false });
    await createdReload;

    expect(unreadCount.value).toBe(0);
  });

  it('auto-read masks the row an in-flight created-reload brings back, so the badge never bumps (§2 Row 1)', async () => {
    seedUnread([]);
    (markNotificationRead as Mock).mockResolvedValue({ success: true });
    const created = deferred<NotifResponse>();
    (getNotifications as Mock).mockReturnValueOnce(created.promise);

    const createdReload = loadUnreadNotifications();
    markReadOptimistic('x');
    expect(unreadCount.value).toBe(0);

    created.resolve({ notifications: [makeNotification('x', false)], unread_count: 1, has_more: false });
    await createdReload;

    expect(unreadCount.value).toBe(0);
  });

  it('lets a local mark-all-read mask what an older in-flight reload brings back', async () => {
    seedUnread(makeUnread(5));
    expect(unreadCount.value).toBe(5);
    const stale = deferred<NotifResponse>();
    (getNotifications as Mock).mockReturnValueOnce(stale.promise);
    (markAllNotificationsRead as Mock).mockResolvedValueOnce(undefined);

    const staleReload = loadUnreadNotifications();
    await markAllRead();
    expect(unreadCount.value).toBe(0);

    stale.resolve({ notifications: makeUnread(5), unread_count: 5, has_more: false });
    await staleReload;

    expect(unreadCount.value).toBe(0);
  });

  it('mark-all-read clears the set on the tap, before the server answers', async () => {
    seedUnread(makeUnread(2));
    servedNotifications.value = { status: 'loaded', data: makeUnread(2) };
    const post = deferred<void>();
    (markAllNotificationsRead as Mock).mockReturnValueOnce(post.promise);

    const pending = markAllRead();

    expect(unreadCount.value).toBe(0);
    expect(loadedRows(notifications.value).every((n) => n.read)).toBe(true);
    post.resolve();
    await pending;
    expect(unreadCount.value).toBe(0);
  });

  it('a failed mark-all-read puts the rows back and says so', async () => {
    seedUnread(makeUnread(2));
    servedNotifications.value = { status: 'loaded', data: makeUnread(2) };
    toasts.value = [];
    (markAllNotificationsRead as Mock).mockRejectedValueOnce(new Error('request timed out'));

    await markAllRead();

    expect(unreadCount.value).toBe(2);
    expect(loadedRows(notifications.value).some((n) => n.read)).toBe(false);
    expect(toasts.value.some((t) => t.type === 'error' && /mark all as read/i.test(t.message))).toBe(true);
  });

  it('a failed mark-all-read keeps a newer read of the same row held', async () => {
    seedUnread(makeUnread(2));
    let reject!: (e: unknown) => void;
    (markAllNotificationsRead as Mock).mockReturnValueOnce(new Promise<void>((_, r) => { reject = r; }));
    (markNotificationRead as Mock).mockReturnValueOnce(new Promise(() => {}));

    const pending = markAllRead();
    markReadOptimistic('u0');
    reject(new Error('request timed out'));
    await pending;

    expect(loadedRows(unreadNotifications.value).map((n) => n.id)).toEqual(['u1']);
  });

  it('a cold-start load that lands after the read never shows the read row', async () => {
    servedUnreadNotifications.value = { status: 'not-loaded' };
    (markNotificationRead as Mock).mockReturnValueOnce(new Promise(() => {}));
    const startup = deferred<NotifResponse>();
    (getNotifications as Mock).mockReturnValueOnce(startup.promise);

    const inFlight = loadUnreadNotifications();
    markReadOptimistic('x');
    startup.resolve({
      notifications: [makeNotification('x', false), makeNotification('y', false)], unread_count: 2, has_more: false,
    });
    await inFlight;

    expect(unreadCount.value).toBe(1);
    expect(loadedRows(unreadNotifications.value).map((n) => n.id)).toEqual(['y']);
  });
});

describe('a read holds until the server has acknowledged it', () => {
  const ids = (list: Loadable<Notification[]>) => loadedRows(list).map((n) => n.id);

  beforeEach(() => {
    seedUnread([makeNotification('x', false), makeNotification('y', false)]);
    servedNotifications.value = { status: 'loaded', data: [makeNotification('x', false), makeNotification('y', false)] };
    (getNotifications as Mock).mockReset();
    (markNotificationRead as Mock).mockReset();
    (markAllNotificationsRead as Mock).mockReset();
  });

  it('a load started after the read, but served before the POST committed, keeps the row hidden', async () => {
    const post = deferred<{ success: boolean }>();
    (markNotificationRead as Mock).mockReturnValueOnce(post.promise);
    const served = deferred<NotifResponse>();
    (getNotifications as Mock).mockReturnValueOnce(served.promise);

    markReadOptimistic('x');
    const load = loadUnreadNotifications();
    served.resolve({ notifications: [makeNotification('x', false), makeNotification('y', false)], unread_count: 2, has_more: false });
    await load;

    expect(ids(unreadNotifications.value)).toEqual(['y']);
  });

  it('a load started before the acknowledgement stays subtracted after the POST resolves', async () => {
    const post = deferred<{ success: boolean }>();
    (markNotificationRead as Mock).mockReturnValueOnce(post.promise);
    const served = deferred<NotifResponse>();
    (getNotifications as Mock).mockReturnValueOnce(served.promise);

    markReadOptimistic('x');
    const load = loadUnreadNotifications();
    post.resolve({ success: true });
    await post.promise;
    served.resolve({ notifications: [makeNotification('x', false), makeNotification('y', false)], unread_count: 2, has_more: false });
    await load;

    expect(ids(unreadNotifications.value)).toEqual(['y']);
  });

  it('a load started after the acknowledgement is the server truth again', async () => {
    (markNotificationRead as Mock).mockResolvedValueOnce({ success: true });
    markReadOptimistic('x');
    await vi.waitFor(() => expect(markNotificationRead).toHaveBeenCalled());
    await Promise.resolve();

    (getNotifications as Mock).mockResolvedValueOnce({ notifications: [makeNotification('y', false)], unread_count: 1, has_more: false });
    await loadUnreadNotifications();

    expect(ids(unreadNotifications.value)).toEqual(['y']);
  });

  it('a newer browse load cannot end the hold while an older unread load is still out', async () => {
    servedUnreadNotifications.value = { status: 'not-loaded' };
    const startup = deferred<NotifResponse>();
    (getNotifications as Mock).mockReturnValueOnce(startup.promise);
    const inFlight = loadUnreadNotifications();
    (markNotificationRead as Mock).mockResolvedValueOnce({ success: true });

    markReadOptimistic('x');
    await vi.waitFor(() => expect(markNotificationRead).toHaveBeenCalled());
    await Promise.resolve();
    (getNotifications as Mock).mockResolvedValueOnce({ notifications: [makeNotification('x', true)], unread_count: 0, has_more: false });
    await loadNotifications();
    startup.resolve({ notifications: [makeNotification('x', false)], unread_count: 1, has_more: false });
    await inFlight;

    expect(unreadCount.value).toBe(0);
  });

  it('a superseded load that lands late still lets an acknowledged read go', async () => {
    servedNotifications.value = { status: 'not-loaded' };
    const older = deferred<NotifResponse>();
    (getNotifications as Mock).mockReturnValueOnce(older.promise);
    (markNotificationRead as Mock).mockResolvedValueOnce({ success: true });

    markReadOptimistic('x');
    const stale = loadUnreadNotifications();
    await vi.waitFor(() => expect(markNotificationRead).toHaveBeenCalled());
    await Promise.resolve();
    (getNotifications as Mock).mockResolvedValueOnce({ notifications: [makeNotification('y', false)], unread_count: 1, has_more: false });
    await loadUnreadNotifications();
    older.resolve({ notifications: [makeNotification('x', false), makeNotification('y', false)], unread_count: 2, has_more: false });
    await stale;

    expect(ids(unreadNotifications.value)).toEqual(['y']);
    expect(pendingNotificationReads.value.size).toBe(0);
  });

  it('a snapshot read that is abandoned stops owing an acknowledged read', async () => {
    servedNotifications.value = { status: 'not-loaded' };
    (markNotificationRead as Mock).mockResolvedValueOnce({ success: true });

    markReadOptimistic('x');
    const snapshotRead = unreadNotificationsSlice.begin();
    await vi.waitFor(() => expect(markNotificationRead).toHaveBeenCalled());
    await Promise.resolve();
    (getNotifications as Mock).mockResolvedValueOnce({ notifications: [makeNotification('y', false)], unread_count: 1, has_more: false });
    await loadUnreadNotifications();
    expect(pendingNotificationReads.value.size).toBe(1);

    unreadNotificationsSlice.abandon?.(snapshotRead);
    expect(pendingNotificationReads.value.size).toBe(0);
  });

  it('a failed read lets the served row show again', async () => {
    let reject!: (e: unknown) => void;
    (markNotificationRead as Mock).mockReturnValueOnce(new Promise((_, r) => { reject = r; }));

    markReadOptimistic('x');
    expect(ids(unreadNotifications.value)).toEqual(['y']);
    reject(new Error('request timed out'));

    await vi.waitFor(() => expect(ids(unreadNotifications.value)).toEqual(['x', 'y']));
  });

  it('a read whose first POST timed out is sent once more, and holds when that one lands', async () => {
    toasts.value = [];
    (markNotificationRead as Mock)
      .mockRejectedValueOnce(new DOMException('Request timed out after 10000ms', 'TimeoutError'))
      .mockResolvedValueOnce({ success: true });

    markReadOptimistic('x');

    await vi.waitFor(() => expect(markNotificationRead).toHaveBeenCalledTimes(2));
    await Promise.resolve();
    expect(ids(unreadNotifications.value)).toEqual(['y']);
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(0);
  });

  it('a refused read is not sent again', async () => {
    (markNotificationRead as Mock).mockRejectedValueOnce(new Error('500 boom'));

    markReadOptimistic('x');

    await vi.waitFor(() => expect(ids(unreadNotifications.value)).toEqual(['x', 'y']));
    expect(markNotificationRead).toHaveBeenCalledTimes(1);
  });

  it('a read that fails names the notification by its title, never by its id', async () => {
    toasts.value = [];
    const timeout = () => new DOMException('Request timed out after 10000ms', 'TimeoutError');
    (markNotificationRead as Mock).mockRejectedValueOnce(timeout()).mockRejectedValueOnce(timeout());

    markReadOptimistic('x');

    await vi.waitFor(() => expect(toasts.value).toHaveLength(1));
    expect(toasts.value[0].message).toBe('Could not mark "Notification x" read: request timed out');
  });

  it('a read of a row this page never loaded falls back to its id', async () => {
    toasts.value = [];
    (markNotificationRead as Mock).mockRejectedValueOnce(new Error('500 boom'));

    markReadOptimistic('never-loaded');

    await vi.waitFor(() => expect(toasts.value).toHaveLength(1));
    expect(toasts.value[0].message).toBe('Could not mark notification never-loaded read: 500 boom');
  });

  it('the browse list keeps the row read across a load served before the commit', async () => {
    const post = deferred<{ success: boolean }>();
    (markNotificationRead as Mock).mockReturnValueOnce(post.promise);
    const served = deferred<NotifResponse>();
    (getNotifications as Mock).mockReturnValueOnce(served.promise);

    markReadOptimistic('x');
    const load = loadNotifications();
    served.resolve({ notifications: [makeNotification('x', false), makeNotification('y', false)], unread_count: 2, has_more: false });
    await load;

    expect(loadedRows(notifications.value).find((n) => n.id === 'x')?.read).toBe(true);
    expect(loadedRows(notifications.value).find((n) => n.id === 'y')?.read).toBe(false);
  });

  it('mark all read reloads the visible browse list, so an earlier load cannot show a row it never held', async () => {
    activeMenuItem.value = 'notifications';
    notificationsFilter.value = 'all';
    panelOverlay.value = null;
    const stale = deferred<NotifResponse>();
    (getNotifications as Mock).mockReturnValueOnce(stale.promise);
    const browse = loadNotifications();
    const post = deferred<void>();
    (markAllNotificationsRead as Mock).mockReturnValueOnce(post.promise);

    const marking = markAllRead();
    (getNotifications as Mock).mockResolvedValue({
      notifications: [makeNotification('x', true), makeNotification('y', true), makeNotification('z', true)],
      unread_count: 0, has_more: false,
    });
    post.resolve();
    await marking;
    stale.resolve({
      notifications: [makeNotification('x', false), makeNotification('y', false), makeNotification('z', false)],
      unread_count: 3, has_more: false,
    });
    await browse;

    await vi.waitFor(() => expect(loadedRows(notifications.value).find((n) => n.id === 'z')?.read).toBe(true));
  });

  it('mark all read holds every row it cleared across a load served before the commit', async () => {
    const post = deferred<void>();
    (markAllNotificationsRead as Mock).mockReturnValueOnce(post.promise);
    const served = deferred<NotifResponse>();
    (getNotifications as Mock).mockReturnValueOnce(served.promise);

    const marking = markAllRead();
    const load = loadUnreadNotifications();
    served.resolve({ notifications: [makeNotification('x', false), makeNotification('y', false)], unread_count: 2, has_more: false });
    await load;

    expect(unreadCount.value).toBe(0);
    (getNotifications as Mock).mockResolvedValueOnce({ notifications: [], unread_count: 0, has_more: false });
    post.resolve();
    await marking;
    expect(unreadCount.value).toBe(0);
  });
});

describe('loadMoreNotifications: a reload that lands mid-request', () => {
  function page(start: number, n: number): Notification[] {
    return Array.from({ length: n }, (_, i) => ({
      ...makeNotification(`p${start + i}`, true),
      created_at: new Date(Date.UTC(2026, 0, 1, 0, 0, 0) - (start + i) * 1000).toISOString(),
    }));
  }

  beforeEach(() => {
    notificationsFilter.value = 'all';
    notificationsHasMore.value = true;
    notificationsLoadingMore.value = false;
    (getNotifications as Mock).mockReset();
  });

  it('appends the page when nothing moved under it', async () => {
    servedNotifications.value = { status: 'loaded', data: page(0, 3) };
    (getNotifications as Mock).mockResolvedValueOnce({
      notifications: page(3, 2), unread_count: 0, has_more: false,
    });

    await loadMoreNotifications();

    const ids = (notifications.value as Loadable<Notification[]> & { data: Notification[] }).data.map(n => n.id);
    expect(ids).toEqual(['p0', 'p1', 'p2', 'p3', 'p4']);
    expect(notificationsHasMore.value).toBe(false);
  });

  it('re-cursors from the new tail instead of appending across a reload', async () => {
    servedNotifications.value = { status: 'loaded', data: page(0, 4) };
    (getNotifications as Mock).mockImplementationOnce(async () => {
      servedNotifications.value = { status: 'loaded', data: page(0, 2) };
      return { notifications: page(4, 2), unread_count: 0, has_more: true };
    });
    (getNotifications as Mock).mockResolvedValueOnce({
      notifications: page(2, 2), unread_count: 0, has_more: true,
    });

    await loadMoreNotifications();

    const ids = (notifications.value as Loadable<Notification[]> & { data: Notification[] }).data.map(n => n.id);
    expect(ids).toEqual(['p0', 'p1', 'p2', 'p3']);
    expect(getNotifications).toHaveBeenCalledTimes(2);
    expect((getNotifications as Mock).mock.calls[1][0]).toMatchObject({
      before: new Date(page(0, 2)[1].created_at).getTime() / 1000,
    });
  });

  it('never doubles a row the reload already carried', async () => {
    servedNotifications.value = { status: 'loaded', data: page(0, 2) };
    (getNotifications as Mock).mockResolvedValueOnce({
      notifications: [...page(1, 1), ...page(2, 1)], unread_count: 0, has_more: false,
    });

    await loadMoreNotifications();

    const ids = (notifications.value as Loadable<Notification[]> & { data: Notification[] }).data.map(n => n.id);
    expect(ids).toEqual(['p0', 'p1', 'p2']);
  });

  it('gives up after a bounded number of retries rather than spinning', async () => {
    servedNotifications.value = { status: 'loaded', data: page(0, 3) };
    (getNotifications as Mock).mockImplementation(async () => {
      servedNotifications.value = { status: 'loaded', data: page(0, 3) };
      return { notifications: page(3, 1), unread_count: 0, has_more: true };
    });

    await loadMoreNotifications();

    expect((getNotifications as Mock).mock.calls.length).toBeLessThanOrEqual(3);
    expect(notificationsLoadingMore.value).toBe(false);
  });
});

describe('refreshActiveNotificationsTab', () => {
  it('keeps the list on screen and settles once the new rows land', async () => {
    notificationsFilter.value = 'all';
    servedNotifications.value = { status: 'loaded', data: [makeNotification('old', true)] };
    const pending = deferred<NotifResponse>();
    (getNotifications as Mock).mockReturnValueOnce(pending.promise);

    let settled = false;
    const refresh = refreshActiveNotificationsTab().then(() => { settled = true; });
    await Promise.resolve();
    expect(notifications.value.status).toBe('loaded');
    expect(settled).toBe(false);

    const fresh = makeNotification('new', false);
    pending.resolve({ notifications: [fresh], unread_count: 1, has_more: false });
    await refresh;
    expect(notifications.value).toEqual({ status: 'loaded', data: [fresh] });
  });
});

describe('notification read source generation', () => {
  beforeEach(() => {
    vi.mocked(markNotificationRead).mockClear();
    resetViewDedup();
    servedNotifications.value = { status: 'not-loaded' };
    servedUnreadNotifications.value = { status: 'not-loaded' };
  });
  it('does not mark a reused id from an old or unknown push', async () => {
    const { observeSourceGeneration } = await import('../../utils/restoreSourceGeneration');
    const generation = '11111111-1111-4111-8111-111111111111';
    observeSourceGeneration(generation);
    try {
      for (const source of ['unknown', 'none', '22222222-2222-4222-8222-222222222222']) {
        markReadOptimistic('reused', source);
      }
      await Promise.resolve();
      expect(markNotificationRead).not.toHaveBeenCalled();
      markReadOptimistic('reused', generation);
      expect(markNotificationRead).toHaveBeenCalledTimes(1);
    } finally { observeSourceGeneration(null); }
  });

  it('retains the original generation across a detail GET', async () => {
    const { observeSourceGeneration } = await import('../../utils/restoreSourceGeneration');
    const pending = deferred<Notification>();
    (getNotification as Mock).mockReturnValueOnce(pending.promise);
    const viewing = viewNotification('delayed-source', 'none');
    observeSourceGeneration('11111111-1111-4111-8111-111111111111');
    try {
      pending.resolve(makeNotification('delayed-source', false));
      await viewing;
      expect(markNotificationRead).not.toHaveBeenCalled();
    } finally { observeSourceGeneration(null); }
  });
});
