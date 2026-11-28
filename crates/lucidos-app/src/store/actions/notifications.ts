import { sourceGeneration } from '../../utils/restoreSourceGeneration';
import { batch } from '@preact/signals';
import {
  notifications,
  unreadNotifications,
  servedNotifications,
  servedUnreadNotifications,
  pendingNotificationReads,
  servedNotificationEpochs,
  showToast,
  removeToast,
  connectionStatus,
  notificationsFilter,
  notificationsHasMore,
  notificationsLoadingMore,
  panelOverlay,
  viewingNotification,
  notificationDetailPending,
  activeMenuItem,
  focusedPane,
  type NotificationsFilter,
} from '../store';
import { toFailed, setLoadingIfFresh, type Notification } from '../types';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';
import { readIsPending, type PendingRead } from '../notificationReads';
import { savePreference } from './preferences';
import { syncWorkspaceAppBadge } from './app-badge';
import { revealContentPane } from './pane';
import { pushNavState, replaceNavState } from './navigation';
import {
  getNotifications,
  getNotification,
  markNotificationRead,
  markAllNotificationsRead,
  isTransportError,
  retryTransient,
} from '../../api/client';
import { errorDetail, isAbortError } from '../../utils/errorDetail';
import { createFailureCounter } from '../../utils/failureCounter';
import { isTauri } from '../../utils/platform';
import { nudgeDockBadge } from '../../utils/tauri';

const PAGE_SIZE = 15;

const MAX_PAGE_RETRIES = 3;

const UNREAD_LOAD_LIMIT = 100;

let loadEpoch = 0;

let latestUnreadLoad = 0;
let latestBrowseLoad = 0;

const loadsInFlight = { browse: new Set<number>(), unread: new Set<number>() };

export function _resetNotificationLoadsForTesting(): void {
  loadsInFlight.browse.clear();
  loadsInFlight.unread.clear();
}

function beginLoad(list: keyof typeof loadsInFlight): number {
  const epoch = ++loadEpoch;
  loadsInFlight[list].add(epoch);
  return epoch;
}

let lastReadOwner = 0;

function updatePendingReads(update: (reads: Map<string, PendingRead>) => void): void {
  const next = new Map(pendingNotificationReads.value);
  update(next);
  pendingNotificationReads.value = next;
}

function holdReads(ids: readonly string[]): number {
  const owner = ++lastReadOwner;
  updatePendingReads((reads) => { for (const id of ids) reads.set(id, { owner, ackedAt: null }); });
  syncWorkspaceAppBadge();
  return owner;
}

function acknowledgeReads(ids: readonly string[], owner: number): void {
  const ackedAt = loadEpoch;
  updatePendingReads((reads) => {
    for (const id of ids) if (reads.get(id)?.owner === owner) reads.set(id, { owner, ackedAt });
  });
}

function abandonReads(ids: readonly string[], owner: number): void {
  updatePendingReads((reads) => {
    for (const id of ids) if (reads.get(id)?.owner === owner) reads.delete(id);
  });
  syncWorkspaceAppBadge();
}

function pruneReads(): void {
  const epochs = servedNotificationEpochs.value;
  const owes = (
    inFlight: ReadonlySet<number>,
    list: typeof servedNotifications.value,
    epoch: number,
    ack: number,
  ) => [...inFlight].some((started) => started <= ack)
    || (list.status === 'loaded' && readIsPending(ack, epoch));
  const settled = [...pendingNotificationReads.value].filter(([, { ackedAt }]) =>
    ackedAt !== null
    && !owes(loadsInFlight.browse, servedNotifications.value, epochs.browse, ackedAt)
    && !owes(loadsInFlight.unread, servedUnreadNotifications.value, epochs.unread, ackedAt));
  if (settled.length > 0) updatePendingReads((reads) => { for (const [id] of settled) reads.delete(id); });
}

function serveBrowse(list: typeof servedNotifications.value, epoch: number): void {
  batch(() => {
    servedNotifications.value = list;
    servedNotificationEpochs.value = { ...servedNotificationEpochs.value, browse: epoch };
    pruneReads();
  });
}

function browseListHas(id: string): boolean {
  const list = notifications.value;
  return list.status === 'loaded' && list.data.some((n) => n.id === id);
}

function findLoadedNotification(id: string): Notification | null {
  for (const list of [notifications.value, unreadNotifications.value]) {
    if (list.status !== 'loaded') continue;
    const row = list.data.find((n) => n.id === id);
    if (row) return row;
  }
  return null;
}

function openNotificationDetail(notification: Notification): void {
  panelOverlay.value = { type: 'notification-detail', notification };
  revealContentPane();
  pushNavState();
}

export async function loadNotifications(): Promise<void> {
  const epoch = beginLoad('browse');
  latestBrowseLoad = epoch;
  setLoadingIfFresh(servedNotifications);
  try {
    const data = await getNotifications({
      limit: PAGE_SIZE,
      filter: notificationsFilter.value,
    }).finally(() => loadsInFlight.browse.delete(epoch));
    if (epoch !== latestBrowseLoad) return;
    serveBrowse({ status: 'loaded', data: data.notifications || [] }, epoch);
    notificationsHasMore.value = data.has_more;
  } catch (error) {
    if (epoch === latestBrowseLoad) servedNotifications.value = toFailed(error);
  } finally {
    pruneReads();
  }
}

export async function loadMoreNotifications(): Promise<void> {
  if (notificationsLoadingMore.value || !notificationsHasMore.value) return;

  notificationsLoadingMore.value = true;
  try {
    for (let attempt = 0; attempt < MAX_PAGE_RETRIES; attempt++) {
      const base = servedNotifications.value;
      if (base.status !== 'loaded' || base.data.length === 0) return;
      const tail = base.data[base.data.length - 1];

      const epoch = beginLoad('browse');
      const data = await getNotifications({
        limit: PAGE_SIZE,
        before: new Date(tail.created_at).getTime() / 1000,
        filter: notificationsFilter.value,
      }).finally(() => loadsInFlight.browse.delete(epoch));

      const after = servedNotifications.value;
      if (after.status !== 'loaded') return;
      if (after.data !== base.data) continue;

      const seen = new Set(after.data.map(n => n.id));
      const fresh = (data.notifications || []).filter(n => !seen.has(n.id));
      serveBrowse(
        { status: 'loaded', data: [...after.data, ...fresh] },
        Math.min(epoch, servedNotificationEpochs.value.browse),
      );
      notificationsHasMore.value = data.has_more;
      return;
    }
  } catch (error) {
    showToast(`Failed to load more notifications: ${errorDetail(error)}`, 'error');
  } finally {
    notificationsLoadingMore.value = false;
    pruneReads();
  }
}

export function refreshActiveNotificationsTab(): Promise<void> {
  return notificationsFilter.value === 'unread' ? loadUnreadNotifications() : loadNotifications();
}

export function setNotificationsFilter(filter: NotificationsFilter): void {
  notificationsFilter.value = filter;
  void refreshActiveNotificationsTab();
  void savePreference('notifications_filter', filter);
}

const UNREAD_STALE_TOAST_KEY = 'refresh-unread-count';

const UNREAD_LOAD_TOAST_THRESHOLD = 3;
const unreadLoadFailures = createFailureCounter(UNREAD_LOAD_TOAST_THRESHOLD, () => {
  showToast(
    `Unread count is stale: ${UNREAD_LOAD_TOAST_THRESHOLD} refresh attempts in a row failed`,
    'error',
    { key: UNREAD_STALE_TOAST_KEY },
  );
});

export function loadUnreadNotifications(): Promise<void> {
  return loadSlice('unread_notifications', unreadNotificationsSlice,
    () => getNotifications({ limit: UNREAD_LOAD_LIMIT, filter: 'unread' }));
}

export const unreadNotificationsSlice: SliceOf<'unread_notifications', number> = {
  begin: () => {
    const epoch = beginLoad('unread');
    latestUnreadLoad = epoch;
    return epoch;
  },
  apply: (data, epoch) => {
    loadsInFlight.unread.delete(epoch);
    if (epoch === latestUnreadLoad) {
      batch(() => {
        servedUnreadNotifications.value = { status: 'loaded', data: data.notifications || [] };
        servedNotificationEpochs.value = { ...servedNotificationEpochs.value, unread: epoch };
        pruneReads();
      });
      syncWorkspaceAppBadge();
    }
    unreadLoadFailures.recordSuccess();
    removeToast(UNREAD_STALE_TOAST_KEY);
    pruneReads();
  },
  fail: (e, epoch) => {
    loadsInFlight.unread.delete(epoch);
    countUnreadLoadFailure(e);
    pruneReads();
  },
  abandon: (epoch) => {
    loadsInFlight.unread.delete(epoch);
    pruneReads();
  },
};

function countUnreadLoadFailure(e: unknown): void {
    if (isAbortError(e) || isTransportError(e)) return;
    if (connectionStatus.value !== 'connected') return;
    unreadLoadFailures.recordFailure();
}

export function handleNotificationSSE(): void {
  reloadNotificationLists();
  if (isTauri()) nudgeDockBadge();
}

function reloadNotificationLists(): void {
  void loadUnreadNotifications();
  if (
    activeMenuItem.value === 'notifications' &&
    !viewingNotification.value &&
    notificationsFilter.value === 'all'
  ) {
    void loadNotifications();
  }
}

let _lastViewedId: string | null = null;
let _lastViewedAt = 0;

export function resetViewDedup(): void {
  _lastViewedId = null;
}

function notificationName(id: string): string {
  for (const list of [servedNotifications.value, servedUnreadNotifications.value]) {
    const row = list.status === 'loaded' ? list.data.find((n) => n.id === id) : undefined;
    if (row) return `"${row.title}"`;
  }
  return `notification ${id}`;
}

export function markReadOptimistic(id: string, generation = sourceGeneration() ?? 'none'): void {
  if (generation !== (sourceGeneration() ?? 'none')) return;
  const owner = holdReads([id]);
  retryTransient(() => {
    if (generation !== (sourceGeneration() ?? 'none')) {
      throw new Error('Workspace restored; notification read canceled');
    }
    return markNotificationRead(id);
  })
    .then(() => acknowledgeReads([id], owner))
    .catch((error) => {
      abandonReads([id], owner);
      showToast(`Could not mark ${notificationName(id)} read: ${errorDetail(error)}`, 'error', {
        key: `notification-read-failed-${id}`,
      });
    });
}

export async function viewNotification(id: string, generation = sourceGeneration() ?? 'none'): Promise<void> {
  const now = Date.now();
  if (id === _lastViewedId && now - _lastViewedAt < 10_000) return;
  _lastViewedId = id;
  _lastViewedAt = now;

  try {
    let notification = findLoadedNotification(id);
    if (!notification) {
      notificationDetailPending.value = id;
      revealContentPane();
      notification = await getNotification(id);
    }
    if (!notification) {
      _lastViewedId = null;
      showToast('Notification not found', 'error');
      return;
    }
    openNotificationDetail(notification);
    if (!browseListHas(id)) await loadNotifications();
    markReadOptimistic(id, generation);
  } catch (error) {
    _lastViewedId = null;
    showToast('Failed to load notification: ' + errorDetail(error), 'error');
  } finally {
    if (notificationDetailPending.value === id) notificationDetailPending.value = null;
  }
}

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

export async function navigateAdjacentNotification(
  currentId: string,
  direction: -1 | 1,
): Promise<string | null> {
  let list = notifications.value;
  if (list.status !== 'loaded') return null;
  let index = list.data.findIndex((n) => n.id === currentId);
  if (index < 0) return null;
  let targetIndex = index + direction;

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
  return navigateToNotification(target.id);
}

export function stepViewedNotification(direction: -1 | 1): boolean {
  const detail = viewingNotification.value;
  if (!detail || focusedPane.value !== 'content') return false;
  void navigateAdjacentNotification(detail.id, direction);
  return true;
}

export function onNotificationDetailClosed(): void {
  resetViewDedup();
  if (activeMenuItem.value === 'notifications' && notificationsFilter.value === 'all') {
    void loadNotifications();
  }
}

export async function markAllRead(): Promise<void> {
  const unreadRows = (list: typeof notifications.value) =>
    list.status === 'loaded' ? list.data.filter((n) => !n.read).map((n) => n.id) : [];
  const ids = [...new Set([...unreadRows(unreadNotifications.value), ...unreadRows(notifications.value)])];
  const owner = holdReads(ids);
  try {
    await markAllNotificationsRead();
    acknowledgeReads(ids, owner);
    reloadNotificationLists();
  } catch (error) {
    abandonReads(ids, owner);
    showToast('Failed to mark all as read: ' + errorDetail(error), 'error');
  }
}
