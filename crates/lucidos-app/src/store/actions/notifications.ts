import {
  notifications,
  unreadNotifications,
  showToast,
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

/** Cap for the unread-set load. Unread is naturally small, and the API clamps
 *  `limit` to 100 — so this is the most we pull in one go. A backlog larger than
 *  this renders as "99+" on the badge: accurate enough, and far past any
 *  realistic unread count. */
const UNREAD_LOAD_LIMIT = 100;

/** Monotonic guard over `unreadNotifications` — the single source of truth the
 *  bell badge projects (the `unreadCount` computed is just its length). An async
 *  load applies its result only if no newer operation has claimed since it
 *  started, and every local optimistic mutation (`markReadOptimistic` /
 *  `navigateToNotification` / `markAllRead`) invalidates any in-flight load
 *  before it writes. This is the guard the old per-`unreadCount`-number version
 *  had, except it now protects the notifications THEMSELVES rather than a
 *  separately-fetched count — so the badge can never disagree with the unread
 *  set, and an out-of-order load can't resurrect a stale one. It still covers
 *  the in-app auto-read sequence (notifications.md §4 Row 1): looking at a
 *  source event fires NotificationCreated then NotificationRead back-to-back;
 *  the optimistic mark-read invalidates the created-reload, so the later state
 *  wins regardless of response order. */
let unreadSeq = 0;
function claimUnreadSeq(): number {
  return ++unreadSeq;
}
function isCurrentUnread(seq: number): boolean {
  return seq === unreadSeq;
}
/** Invalidate any in-flight unread load ahead of a local optimistic mutation. */
function invalidateUnreadLoad(): void {
  unreadSeq++;
}

/** Drop one notification from the unread set — the badge falls with it. No-op
 *  on the set's contents if it isn't loaded or the row isn't in it (idempotent).
 *
 *  Invalidates any in-flight load BEFORE the membership check, not after: on the
 *  §4 Row 1 auto-read path the created-reload is still in flight when the row is
 *  marked read, so the row isn't in the set yet. Without superseding that load
 *  here, it would land a beat later and re-add the just-read notification — a
 *  transient badge bump that breaks the §2 Row 1 "no bell badge" contract. */
function removeFromUnread(id: string): void {
  const set = unreadNotifications.value;
  if (set.status !== 'loaded') return;
  invalidateUnreadLoad();
  if (!set.data.some((n) => n.id === id)) return;
  unreadNotifications.value = { status: 'loaded', data: set.data.filter((n) => n.id !== id) };
}

/** Flip a single row to read in the inbox browse list (display only — the badge
 *  is driven by the unread set, not this list). No-op if absent / already read. */
function markBrowseRowRead(id: string): void {
  const browse = notifications.value;
  if (browse.status !== 'loaded') return;
  const row = browse.data.find((n) => n.id === id);
  if (!row || row.read) return;
  notifications.value = {
    status: 'loaded',
    data: browse.data.map((n) => (n.id === id ? { ...n, read: true } : n)),
  };
}

/** Load the first page of notifications using the current filter. This populates
 *  the inbox browse list only; the bell badge derives from the unread set
 *  (`loadUnreadNotifications`), so opening the inbox never reaches for a count. */
export async function loadNotifications(): Promise<void> {
  setLoadingIfFresh(notifications);
  try {
    const data = await getNotifications({
      limit: PAGE_SIZE,
      filter: notificationsFilter.value,
    });
    notifications.value = { status: 'loaded', data: data.notifications || [] };
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

/** Threshold for the unread-load escalation toast. Three consecutive failures
 *  before we bother the user — a single transient failure shouldn't surface,
 *  but a sustained outage should so the user knows the badge is stale. Reset on
 *  the next success. */
const UNREAD_LOAD_TOAST_THRESHOLD = 3;
const unreadLoadFailures = createFailureCounter(UNREAD_LOAD_TOAST_THRESHOLD, () => {
  showToast(
    `Unread count is stale — couldn't reach the engine after ${UNREAD_LOAD_TOAST_THRESHOLD} tries`,
    'error',
    { key: 'refresh-unread-count' },
  );
});

/** Load the unread set — the single source the bell badge derives from. Called
 *  on startup / resume / notification SSE; runs without user intent, so it's
 *  best-effort: individual failures are swallowed (see `.claude/rules/frontend.md`
 *  § "Carve-out: best-effort telemetry") and escalated via a single toast only
 *  after `UNREAD_LOAD_TOAST_THRESHOLD` consecutive failures. */
export async function loadUnreadNotifications(): Promise<void> {
  const seq = claimUnreadSeq();
  try {
    const data = await getNotifications({ limit: UNREAD_LOAD_LIMIT, filter: 'unread' });
    if (isCurrentUnread(seq)) {
      unreadNotifications.value = { status: 'loaded', data: data.notifications || [] };
    }
    unreadLoadFailures.recordSuccess();
  } catch {
    unreadLoadFailures.recordFailure();
  }
}

/** Handle notification SSE events (NotificationCreated/Read/AllRead). Reloads
 *  the unread set so the badge tracks the server, and reloads the inbox browse
 *  list when it's open. Skips the browse reload when the detail modal is open —
 *  the user is navigating through the list and a reload with `filter: 'unread'`
 *  would remove the currently-viewed item, breaking prev/next navigation. */
export function handleNotificationSSE(): void {
  void loadUnreadNotifications();
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
 *  API call. Idempotent — safe to call on already-read rows. Dropping the row
 *  from the unread set makes the badge fall immediately; the server's
 *  NotificationRead broadcast triggers a confirming reload. */
export function markReadOptimistic(id: string): void {
  markBrowseRowRead(id);
  removeFromUnread(id);
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
      markBrowseRowRead(target.id);
      removeFromUnread(target.id);
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

    // Optimistic update: flip every loaded browse row to read...
    const current = notifications.value;
    if (current.status === 'loaded') {
      notifications.value = {
        status: 'loaded',
        data: current.data.map((n) => ({ ...n, read: true })),
      };
    }
    // ...and empty the unread set, which drops the badge to zero. Invalidate
    // any in-flight load first so a stale reload can't resurrect the count.
    invalidateUnreadLoad();
    unreadNotifications.value = { status: 'loaded', data: [] };
  } catch (error) {
    showToast('Failed to mark all as read: ' + errorDetail(error), 'error');
  }
}
