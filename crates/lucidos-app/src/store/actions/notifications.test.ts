import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  notifications,
  unreadNotifications,
  unreadCount,
  notificationsFilter,
  notificationsHasMore,
  notificationsLoadingMore,
  panelOverlay,
  activeMenuItem,
  toasts,
} from '../store';
import type { Notification, Loadable } from '../types';

// Mock the API client to prevent real HTTP calls
vi.mock('../../api/client', () => ({
  getNotifications: vi.fn().mockResolvedValue({ notifications: [], unread_count: 0, has_more: false }),
  getNotification: vi.fn(),
  markNotificationRead: vi.fn().mockResolvedValue({ success: true }),
  markAllNotificationsRead: vi.fn(),
}));

// viewNotification reveals the content pane and pushes a nav-history entry as a
// side effect; neither is what these tests exercise (they pin the browse-list
// load), and both reach for the DOM / localStorage. Stub them out.
vi.mock('./pane', () => ({ revealContentPane: vi.fn() }));
vi.mock('./navigation', () => ({ pushNavState: vi.fn(), replaceNavState: vi.fn() }));

const {
  handleNotificationSSE,
  loadUnreadNotifications,
  markAllRead,
  markReadOptimistic,
  loadNotifications,
  navigateAdjacentNotification,
  viewNotification,
  resetViewDedup,
} = await import('./notifications');
const { getNotifications, getNotification, markNotificationRead, markAllNotificationsRead } = await import('../../api/client');

type Mock = ReturnType<typeof vi.fn>;
type NotifResponse = { notifications: Notification[]; unread_count: number; has_more: boolean };

/** A promise plus its resolver, so a test can hold a GET pending and resolve
 *  it out of order. */
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

/** N unread notifications, ids `u0`..`u{N-1}`. */
function makeUnread(n: number): Notification[] {
  return Array.from({ length: n }, (_, i) => makeNotification(`u${i}`, false));
}

/** Seed the unread set (the bell badge's single source of truth) directly. */
function seedUnread(items: Notification[]): void {
  unreadNotifications.value = { status: 'loaded', data: items };
}

describe('handleNotificationSSE', () => {
  beforeEach(() => {
    activeMenuItem.value = 'notifications';
    notificationsFilter.value = 'unread';
    panelOverlay.value = null;
    notifications.value = {
      status: 'loaded',
      data: [
        makeNotification('a', false),
        makeNotification('b', false),
        makeNotification('c', false),
      ],
    };
  });

  it('does NOT reload the inbox list when a notification detail is open in the panel', () => {
    // User is viewing a notification detail in the content pane
    panelOverlay.value = { type: 'notification-detail', notification: makeNotification('a', true) };

    // SSE event arrives (e.g. NotificationRead)
    handleNotificationSSE();

    // The browse list should NOT be reloaded — items must stay intact for navigation
    expect(notifications.value.status).toBe('loaded');
    if (notifications.value.status === 'loaded') {
      expect(notifications.value.data).toHaveLength(3);
      expect(notifications.value.data.map((n) => n.id)).toEqual(['a', 'b', 'c']);
    }
  });

  it('reloads the inbox list when no detail is open and panel is active', () => {
    panelOverlay.value = null;
    (getNotifications as Mock).mockClear();

    handleNotificationSSE();

    // Existing data stays visible through the refetch round-trip.
    expect(notifications.value.status).toBe('loaded');
    expect(getNotifications).toHaveBeenCalled();
  });

  it('does NOT reload the inbox list when notifications panel is not active', () => {
    activeMenuItem.value = 'files';

    handleNotificationSSE();

    // Should not reload — still 'loaded' with original data
    expect(notifications.value.status).toBe('loaded');
  });
});

// The badge is a pure projection of the unread set — there is no separately
// fetched count to drift from it. These pin that the count is ALWAYS the set's
// length and that local mark-read shrinks the set immediately.
// The detail-panel chevrons walk the inbox list, which is paginated. Stepping
// "older" (next) must not stop at the first loaded page — it loads the next page
// and continues, so navigation is bounded only by how many notifications exist.
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
    // The loaded row IS the full notification (the list query selects identical
    // columns to the single-notification GET), so stepping must render straight
    // from memory — a per-chevron getNotification round-trip was the iOS-PWA lag.
    notifications.value = {
      status: 'loaded',
      data: [makeNotification('a', true), makeNotification('b', true), makeNotification('c', true)],
    };

    const id = await navigateAdjacentNotification('a', 1);

    expect(id).toBe('b');
    expect(getNotifications).not.toHaveBeenCalled(); // already loaded — no page fetch
    expect(getNotification).not.toHaveBeenCalled();  // rendered from memory — no detail GET
    // The overlay carries the full in-memory row (incl. its message body), not a
    // re-fetched copy.
    expect(panelOverlay.value).toMatchObject({
      type: 'notification-detail',
      notification: { id: 'b', message: 'Message b' },
    });
  });

  it('marks an unread stepped-to row read in the background without a detail GET', async () => {
    notifications.value = {
      status: 'loaded',
      data: [makeNotification('a', true), makeNotification('b', false)],
    };

    const id = await navigateAdjacentNotification('a', 1);

    expect(id).toBe('b');
    expect(getNotification).not.toHaveBeenCalled();         // never gated on a GET
    expect(markNotificationRead).toHaveBeenCalledWith('b'); // fire-and-forget read flip
    // The browse row flips read optimistically (display only).
    if (notifications.value.status === 'loaded') {
      expect(notifications.value.data.find((n) => n.id === 'b')?.read).toBe(true);
    }
  });

  it('does NOT POST a read flip when stepping to an already-read row', async () => {
    notifications.value = {
      status: 'loaded',
      data: [makeNotification('a', true), makeNotification('b', true)],
    };

    await navigateAdjacentNotification('a', 1);

    expect(markNotificationRead).not.toHaveBeenCalled();
  });

  it('loads the next page when stepping older past the loaded boundary, then steps into it', async () => {
    notifications.value = {
      status: 'loaded',
      data: [makeNotification('a', true), makeNotification('b', true)],
    };
    notificationsHasMore.value = true;
    // loadMoreNotifications appends the next page (cursor-based).
    (getNotifications as Mock).mockResolvedValueOnce({
      notifications: [makeNotification('c', true), makeNotification('d', true)],
      unread_count: 0,
      has_more: false,
    });

    const id = await navigateAdjacentNotification('b', 1);

    expect(getNotifications).toHaveBeenCalledTimes(1); // pulled the next page
    expect(getNotification).not.toHaveBeenCalled();    // stepped into it from memory
    expect(id).toBe('c'); // stepped into the freshly-loaded page
    if (notifications.value.status === 'loaded') {
      expect(notifications.value.data.map((n) => n.id)).toEqual(['a', 'b', 'c', 'd']);
    }
  });

  it('does not step past the last loaded item when the server has no more pages', async () => {
    notifications.value = {
      status: 'loaded',
      data: [makeNotification('a', true), makeNotification('b', true)],
    };
    notificationsHasMore.value = false;

    const id = await navigateAdjacentNotification('b', 1);

    expect(id).toBeNull();
    expect(getNotifications).not.toHaveBeenCalled();
  });

  it('never fetches a page in the newer (prev) direction — the newest is always loaded', async () => {
    notifications.value = {
      status: 'loaded',
      data: [makeNotification('a', true), makeNotification('b', true)],
    };
    notificationsHasMore.value = true; // irrelevant to prev

    const id = await navigateAdjacentNotification('a', -1);

    expect(id).toBeNull(); // 'a' is index 0 — nothing newer
    expect(getNotifications).not.toHaveBeenCalled();
  });
});

// A push tap / deep link opens a notification detail via viewNotification
// WITHOUT the user ever having opened the Notifications panel — so the inbox
// browse list is unloaded and the detail's prev/next chevrons (which walk that
// list) would sit permanently disabled. viewNotification must load the list so
// the chevrons can step the inbox.
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
    notifications.value = { status: 'not-loaded' };
    (getNotification as Mock).mockResolvedValueOnce(makeNotification('x', false));
    // The list load runs while the row is still unread server-side, so it comes
    // back under either filter; here it returns the row plus an older sibling.
    (getNotifications as Mock).mockResolvedValueOnce({
      notifications: [makeNotification('x', false), makeNotification('y', false)],
      unread_count: 2,
      has_more: false,
    });

    await viewNotification('x');

    // The detail opened...
    expect(panelOverlay.value).toMatchObject({
      type: 'notification-detail',
      notification: { id: 'x' },
    });
    // ...and the browse list got loaded and now holds the viewed row, so the
    // chevrons have an inbox to walk (currentIndex !== -1).
    expect(getNotifications).toHaveBeenCalledTimes(1);
    // Cast defeats TS flow-narrowing: the test body's last direct assignment was
    // `{ status: 'not-loaded' }`, but viewNotification mutated it across the await.
    const list = notifications.value as Loadable<Notification[]>;
    if (list.status === 'loaded') {
      expect(list.data.some((n) => n.id === 'x')).toBe(true);
    } else {
      throw new Error('expected the browse list to be loaded');
    }
    // The viewed row is marked read in place — it stays in the list (chevrons
    // still resolve it), it isn't dropped.
    expect(markNotificationRead).toHaveBeenCalledWith('x');
  });

  it('does NOT reload when the browse list already holds the row (in-app open)', async () => {
    notifications.value = {
      status: 'loaded',
      data: [makeNotification('x', false), makeNotification('y', false)],
    };
    (getNotification as Mock).mockResolvedValueOnce(makeNotification('x', false));

    await viewNotification('x');

    expect(panelOverlay.value).toMatchObject({ type: 'notification-detail', notification: { id: 'x' } });
    // List already has the row — no redundant page fetch.
    expect(getNotifications).not.toHaveBeenCalled();
  });
});

describe('the bell badge is derived from the unread set', () => {
  beforeEach(() => {
    seedUnread([]);
    notifications.value = { status: 'not-loaded' };
    (getNotifications as Mock).mockReset();
    (markNotificationRead as Mock).mockReset();
    (markNotificationRead as Mock).mockResolvedValue({ success: true });
  });

  it('reports the number of unread notifications in state', () => {
    seedUnread(makeUnread(3));
    expect(unreadCount.value).toBe(3);
    seedUnread([]);
    expect(unreadCount.value).toBe(0);
    unreadNotifications.value = { status: 'not-loaded' };
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
    expect(getNotifications).not.toHaveBeenCalled(); // no fetch needed to drop the badge
  });

  it('loadNotifications (inbox browse) never touches the badge', async () => {
    seedUnread(makeUnread(2)); // badge = 2 from the unread set
    // The browse fetch returns an all-read page (count is irrelevant to it now).
    (getNotifications as Mock).mockResolvedValueOnce({
      notifications: [makeNotification('a', true), makeNotification('b', true)],
      unread_count: 999,
      has_more: false,
    });

    await loadNotifications();

    // Badge stays tied to the unread set — the browse's unread_count is ignored,
    // so an all-read inbox page can never strand a stale positive badge.
    expect(unreadCount.value).toBe(2);
    expect(notifications.value.status).toBe('loaded');
  });
});

describe('loadUnreadNotifications failure handling', () => {
  beforeEach(async () => {
    toasts.value = [];
    seedUnread([]);
    // Reset the module-level failure counter — every test starts with a
    // successful load so failures are independent across tests.
    (getNotifications as Mock).mockReset();
    (getNotifications as Mock).mockResolvedValueOnce({
      notifications: [], unread_count: 0, has_more: false,
    });
    await loadUnreadNotifications();
    toasts.value = [];
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

    // 4th, 5th failures don't re-toast — same outage, one notice.
    await loadUnreadNotifications();
    await loadUnreadNotifications();
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(1);
  });

  it('does not count a browser-cancelled AbortError toward the threshold', async () => {
    // No manual AbortController on this path — an AbortError is the browser
    // cancelling the in-flight fetch on an iOS PWA freeze / radio handoff. It
    // carries no reachability signal, so it must not push the counter toward the
    // "Unread count is stale — couldn't reach the engine" escalation. A genuine
    // unreachable engine fires TimeoutError / a transport TypeError, which still
    // counts (covered by the threshold test above).
    (getNotifications as Mock).mockRejectedValue(new DOMException('aborted', 'AbortError'));

    await loadUnreadNotifications();
    await loadUnreadNotifications();
    await loadUnreadNotifications();
    await loadUnreadNotifications();
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
    await loadUnreadNotifications(); // success — counter resets
    expect(unreadCount.value).toBe(7);

    // Two more failures shouldn't trip the threshold yet (we're back to 0).
    await loadUnreadNotifications();
    await loadUnreadNotifications();
    expect(toasts.value.filter((t) => t.type === 'error')).toHaveLength(0);
  });
});

// The unread set must stay in sync even when two loads are in flight at once.
// The canonical case is an in-app auto-read (notifications.md §4 Row 1): the same
// page sees NotificationCreated (the server now counts the row) and, a beat
// later, NotificationRead (the server flipped it). Each fires a reload; if the
// stale created-reload resolves LAST it must not strand the badge one too high.
describe('unread set is resilient to out-of-order responses', () => {
  beforeEach(() => {
    seedUnread([]);
    (getNotifications as Mock).mockReset();
    (markAllNotificationsRead as Mock).mockReset();
  });

  it('discards a stale created-reload that resolves after the read-reload', async () => {
    // #1 created-reload — held pending, will return the pre-read set (1 unread).
    // #2 read-reload — resolves immediately to an empty set.
    const created = deferred<NotifResponse>();
    (getNotifications as Mock)
      .mockReturnValueOnce(created.promise)
      .mockResolvedValueOnce({ notifications: [], unread_count: 0, has_more: false });

    const createdReload = loadUnreadNotifications(); // claims seq N
    const readReload = loadUnreadNotifications();     // claims seq N+1 (the latest)
    await readReload;
    expect(unreadCount.value).toBe(0);

    // The stale created-reload now lands with the old (non-empty) set.
    created.resolve({ notifications: [makeNotification('x', false)], unread_count: 1, has_more: false });
    await createdReload;

    // It must NOT bounce the badge back to 1 — the read-reload was issued later.
    expect(unreadCount.value).toBe(0);
  });

  it('auto-read invalidates an in-flight created-reload so the badge never bumps (§2 Row 1)', async () => {
    // Set is loaded + empty (badge 0). A NotificationCreated SSE has just kicked
    // off a reload that WILL return the new unread row — but it is still in
    // flight when the user (looking at the source event) auto-reads it.
    seedUnread([]);
    (markNotificationRead as Mock).mockResolvedValue({ success: true });
    const created = deferred<NotifResponse>();
    (getNotifications as Mock).mockReturnValueOnce(created.promise);

    const createdReload = loadUnreadNotifications(); // seq N, in-flight (will return [x])
    markReadOptimistic('x'); // removeFromUnread invalidates the in-flight reload
    expect(unreadCount.value).toBe(0);

    // The created-reload lands AFTER the auto-read with the pre-read set.
    created.resolve({ notifications: [makeNotification('x', false)], unread_count: 1, has_more: false });
    await createdReload;

    // It must NOT bump the badge to 1 — the auto-read superseded it.
    expect(unreadCount.value).toBe(0);
  });

  it('lets a local mark-all-read invalidate an older in-flight reload', async () => {
    seedUnread(makeUnread(5));
    expect(unreadCount.value).toBe(5);
    const stale = deferred<NotifResponse>();
    (getNotifications as Mock).mockReturnValueOnce(stale.promise);
    (markAllNotificationsRead as Mock).mockResolvedValueOnce(undefined);

    const staleReload = loadUnreadNotifications(); // claims seq, held pending (would return 5)
    await markAllRead();                            // local clear — invalidates the in-flight reload
    expect(unreadCount.value).toBe(0);

    stale.resolve({ notifications: makeUnread(5), unread_count: 5, has_more: false });
    await staleReload;

    // The superseded reload must not resurrect the pre-read set.
    expect(unreadCount.value).toBe(0);
  });
});
