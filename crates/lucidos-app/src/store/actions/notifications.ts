import {
  notifications,
  unreadNotifications,
  showToast,
  notificationsFilter,
  notificationsHasMore,
  notificationsLoadingMore,
  panelOverlay,
  viewingNotification,
  activeMenuItem,
} from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import { savePreference } from './preferences';
import { revealContentPane } from './pane';
import { pushNavState, replaceNavState } from './navigation';
import {
  getNotifications,
  getNotification,
  markNotificationRead,
  markAllNotificationsRead,
} from '../../api/client';
import { errorDetail, isAbortError } from '../../utils/errorDetail';
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

/** True when the inbox browse list is loaded AND already holds this row — i.e.
 *  the detail's prev/next chevrons (which walk `notifications`) have a list with
 *  this notification in it to step from. */
function browseListHas(id: string): boolean {
  const list = notifications.value;
  return list.status === 'loaded' && list.data.some((n) => n.id === id);
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
  } catch (e) {
    // A browser-cancelled fetch (iOS PWA freeze / radio handoff) rejects with
    // AbortError — there's no manual AbortController here, so it carries no
    // reachability signal. Don't count it toward the escalation threshold, or a
    // few background-resume cancellations falsely trip "Unread count is stale —
    // couldn't reach the engine". A genuine unreachable engine fires
    // TimeoutError / a transport TypeError, which still counts. The next resume
    // re-syncs the badge.
    if (isAbortError(e)) return;
    unreadLoadFailures.recordFailure();
  }
}

/** Handle notification SSE events (NotificationCreated/Read/AllRead). Reloads
 *  the unread set so the badge tracks the server, and reloads the inbox browse
 *  list when it's open. Skips the browse reload when a notification detail is
 *  open in the panel — the user is navigating through the list and a reload with
 *  `filter: 'unread'` would remove the currently-viewed item, breaking prev/next
 *  navigation. */
export function handleNotificationSSE(): void {
  void loadUnreadNotifications();
  if (activeMenuItem.value === 'notifications' && !viewingNotification.value) {
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
  // Stamp the dedup guard synchronously, before the await, so a near-simultaneous
  // second fire (SW postMessage + URL-param cold start, both for one tap) bails on
  // the guard instead of racing a second fetch. Cleared on failure below so a
  // failed fetch never blocks a re-tap — otherwise the retry silently no-ops for
  // 10s (no detail panel, no second toast — the tap looks dead).
  _lastViewedId = id;
  _lastViewedAt = now;
  try {
    const notification = await getNotification(id);
    if (notification) {
      // Open the detail in the content pane (not a modal): set the overlay,
      // reveal the pane, and push a nav entry so panel Back returns to the
      // inbox list and a reload restores the open detail. Mirrors openUrl /
      // openFilePreview.
      panelOverlay.value = { type: 'notification-detail', notification };
      revealContentPane();
      pushNavState();
      // Deep-link / push-tap opens land here without the inbox browse list ever
      // having been loaded (the user never opened the Notifications panel), so
      // the detail's prev/next chevrons would sit permanently disabled — they
      // walk `notifications` and resolve currentIndex === -1. Load it now when it
      // doesn't already hold this row. Load BEFORE marking read: the just-pushed
      // row is still unread server-side, so it's returned under either filter
      // ('all' or 'unread'); the subsequent mark-read only flips the loaded row
      // in place (markBrowseRowRead never removes it), so currentIndex stays
      // valid and the chevrons can walk the inbox. In-app opens from the panel
      // already have the list loaded with this row, so they skip the fetch.
      if (!browseListHas(id)) await loadNotifications();
      markReadOptimistic(id);
    }
  } catch (error) {
    _lastViewedId = null;
    showToast('Failed to load notification: ' + errorDetail(error), 'error');
  }
}

/** Walk the panel detail to another notification by id (prev/next). Owns the
 *  overlay write the detail component must not do inline. Replaces the current
 *  nav entry in place (`replaceNavState`) so the whole detail-viewing session
 *  is a single history slot and panel Back returns to the inbox list, not
 *  through each notification stepped over. Returns the stepped-to id, or null
 *  when the target isn't in the loaded list.
 *
 *  Renders straight from the in-memory list row — no detail GET. The inbox list
 *  query selects the IDENTICAL columns the single-notification GET does (id,
 *  title, message, tap, event_id, thread_id, app_id, read, created_at), so the
 *  loaded row IS the full notification; re-fetching it added a network
 *  round-trip of lag to every chevron tap (badly felt on an iOS PWA over a slow
 *  link). Mark-read is fire-and-forget via `markReadOptimistic` (optimistic
 *  local cache update + best-effort POST) — it never gated rendering and is a
 *  no-op on already-read rows, so the "don't POST for already-read" optimization
 *  is preserved by the `!target.read` guard. */
export function navigateToNotification(targetId: string): string | null {
  const list = notifications.value;
  const target = list.status === 'loaded'
    ? list.data.find((n) => n.id === targetId)
    : undefined;
  if (!target) return null;

  panelOverlay.value = { type: 'notification-detail', notification: target };
  replaceNavState();
  if (!target.read) markReadOptimistic(target.id);
  return target.id;
}

/** Walk the panel detail to the adjacent notification (prev = -1 / next = +1).
 *  Resolves the target by offset within the loaded inbox list, but in the
 *  "older" (next) direction it transparently pulls the next page first when the
 *  current item is the last loaded one and the server has more — so chevron
 *  navigation walks the ENTIRE inbox, not just the first loaded page. The "newer"
 *  (prev) direction needs no load-more: the list is always loaded newest-first,
 *  so index 0 is the newest notification and there is nothing newer to fetch.
 *  Returns the loaded id, or null when there is no adjacent notification. */
export async function navigateAdjacentNotification(
  currentId: string,
  direction: -1 | 1,
): Promise<string | null> {
  let list = notifications.value;
  if (list.status !== 'loaded') return null;
  let index = list.data.findIndex((n) => n.id === currentId);
  if (index < 0) return null;
  let targetIndex = index + direction;

  // Stepping older past the loaded boundary: fetch the next page, then re-resolve
  // (loadMoreNotifications appends, so the current item's index is unchanged, but
  // re-find defensively in case a concurrent reload reshaped the list).
  if (direction === 1 && targetIndex >= list.data.length && notificationsHasMore.value) {
    await loadMoreNotifications();
    list = notifications.value;
    if (list.status !== 'loaded') return null;
    index = list.data.findIndex((n) => n.id === currentId);
    if (index < 0) return null;
    targetIndex = index + direction;
  }

  const target = list.data[targetIndex];
  if (!target) return null;
  // Synchronous after this point — `navigateToNotification` renders from memory.
  // The function stays async only for the page-boundary `loadMoreNotifications`
  // await above (one fetch every PAGE_SIZE items, not per tap).
  return navigateToNotification(target.id);
}

/** Run when the notification detail panel closes — the overlay is cleared by
 *  panel Back nav, a menu switch, or any restore path, so there is no single
 *  call site to hang this on. Driven by an effect on `viewingNotification` in
 *  store/effects.ts (mirrors the `lastPreviewFile` reset there). Resets the
 *  view-dedup guard so the same notification can be reopened immediately, and
 *  refreshes the inbox list when it's the active panel so a now-read row drops
 *  under the 'unread' filter (what the former modal's close handler did). */
export function onNotificationDetailClosed(): void {
  resetViewDedup();
  if (activeMenuItem.value === 'notifications') void loadNotifications();
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
