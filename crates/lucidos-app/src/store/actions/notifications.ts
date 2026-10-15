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
} from '../../api/client';
import { errorDetail, isAbortError } from '../../utils/errorDetail';
import { createFailureCounter } from '../../utils/failureCounter';
import { isTauri } from '../../utils/platform';
import { nudgeDockBadge } from '../../utils/tauri';

const PAGE_SIZE = 15;

/** How many times `loadMoreNotifications` re-cursors when a concurrent reload
 *  moves the list under its request. Bounded so a busy arrival stream cannot
 *  spin: giving up leaves the list correct, just one page short. */
const MAX_PAGE_RETRIES = 3;

/** Cap for the unread-set load. Unread is naturally small, and the API clamps
 *  `limit` to 100 — so this is the most we pull in one go. A backlog larger than
 *  this renders as "99+" on the badge: accurate enough, and far past any
 *  realistic unread count. */
const UNREAD_LOAD_LIMIT = 100;

/** Every load takes the next epoch when it starts. A read acknowledged at
 *  epoch E is reflected by any load whose epoch is above E, and by no other. */
let loadEpoch = 0;

/** The newest load of each list. An older one landing later is discarded,
 *  so an out-of-order response can never put back a list a newer load
 *  replaced. */
let latestUnreadLoad = 0;
let latestBrowseLoad = 0;

/** The epochs of the loads still out, per list. A read stays pending while
 *  any of them started before its acknowledgement, since that load will
 *  serve the row unread. */
const loadsInFlight = { browse: new Set<number>(), unread: new Set<number>() };

/** Forget the loads a test left hanging, so they owe no reads in the next. */
export function _resetNotificationLoadsForTesting(): void {
  loadsInFlight.browse.clear();
  loadsInFlight.unread.clear();
}

function beginLoad(list: keyof typeof loadsInFlight): number {
  const epoch = ++loadEpoch;
  loadsInFlight[list].add(epoch);
  return epoch;
}

/** Names each request that holds reads, so it settles only its own. */
let lastReadOwner = 0;

function updatePendingReads(update: (reads: Map<string, PendingRead>) => void): void {
  const next = new Map(pendingNotificationReads.value);
  update(next);
  pendingNotificationReads.value = next;
}

/** Hide these rows as read until the server acknowledges it (ADR 0437). The
 *  unread set and the badge fall at once, with no load cancelled: whatever a
 *  load in flight brings back is masked the same way. Returns the owner that
 *  settles the hold. */
function holdReads(ids: readonly string[]): number {
  const owner = ++lastReadOwner;
  updatePendingReads((reads) => { for (const id of ids) reads.set(id, { owner, ackedAt: null }); });
  // Re-assert the app-icon badge even when nothing moved locally. A tap on a
  // push whose row this device never held (frozen page, dropped SSE) drops
  // nothing, but the icon still carries the count the push wrote.
  syncWorkspaceAppBadge();
  return owner;
}

/** The server committed `owner`'s reads: only loads started before now still
 *  owe them a mask. */
function acknowledgeReads(ids: readonly string[], owner: number): void {
  const ackedAt = loadEpoch;
  updatePendingReads((reads) => {
    for (const id of ids) if (reads.get(id)?.owner === owner) reads.set(id, { owner, ackedAt });
  });
}

/** The server refused `owner`'s reads, so the served rows show again, unless
 *  a later request on the same row holds it now. */
function abandonReads(ids: readonly string[], owner: number): void {
  updatePendingReads((reads) => {
    for (const id of ids) if (reads.get(id)?.owner === owner) reads.delete(id);
  });
  syncWorkspaceAppBadge();
}

/** Forget each acknowledged read that every loaded list already reflects.
 *  Every load runs it as it ends, since a load that lost the race or failed
 *  also stops owing its reads. */
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

/** Record what a load served, with the epoch it started at. */
function serveBrowse(list: typeof servedNotifications.value, epoch: number): void {
  batch(() => {
    servedNotifications.value = list;
    servedNotificationEpochs.value = { ...servedNotificationEpochs.value, browse: epoch };
    pruneReads();
  });
}

/** True when the inbox browse list is loaded AND already holds this row — i.e.
 *  the detail's prev/next chevrons (which walk `notifications`) have a list with
 *  this notification in it to step from.
 *
 *  Deliberately NOT the same question as `findLoadedNotification`: this one is
 *  about the CHEVRONS' list, which must be `notifications` specifically (the
 *  unread set drops a row the instant it's marked read, so chevrons walking it
 *  would die on the open). `findLoadedNotification` asks whether we hold the row
 *  at all, from either list. Keep them separate. */
function browseListHas(id: string): boolean {
  const list = notifications.value;
  return list.status === 'loaded' && list.data.some((n) => n.id === id);
}

/** The already-loaded row for `id`, from whichever list holds it, or null.
 *
 *  Both lists carry WHOLE notifications, not summaries: the inbox list query
 *  (`NotificationStore::get_filtered`) and the single-row query (`get_by_id`)
 *  select an identical column list and serialize the same `Notification`, so a
 *  loaded row IS what `GET /api/v1/notification?id=` would return. Re-fetching
 *  it buys nothing and costs a full round-trip (400-800ms on an iOS PWA over
 *  Tailscale, 1100-1800ms on the first packet after the phone radio resumes,
 *  see system-knowhow/notifications.md §3), spent before ANY pixel moves, since
 *  the overlay/reveal/nav-push all sit behind the await. That is the reported
 *  "lag opening a notification, with no user feedback".
 *
 *  This is the same call `navigateToNotification` already makes for the detail's
 *  prev/next chevrons (see its doc comment); the primary open path had been left
 *  on the fetch. The unread set is checked too because the "Unread" tab renders
 *  it rather than the browse list (since the badge/list single-sourcing), so on
 *  that tab it is the ONLY list holding the row the user just tapped. */
function findLoadedNotification(id: string): Notification | null {
  for (const list of [notifications.value, unreadNotifications.value]) {
    if (list.status !== 'loaded') continue;
    const row = list.data.find((n) => n.id === id);
    if (row) return row;
  }
  return null;
}

/** Land a notification in the content pane: overlay, reveal, nav entry.
 *  Shared by the memory-first open and the fetched one so the two can't drift.
 *  A missing `revealContentPane` is a silent no-op on mobile and a missing
 *  `pushNavState` breaks panel Back (see `.claude/rules/frontend.md`). */
function openNotificationDetail(notification: Notification): void {
  panelOverlay.value = { type: 'notification-detail', notification };
  revealContentPane();
  pushNavState();
}

/** Load the first page of notifications using the current filter. This populates
 *  the inbox browse list only; the bell badge derives from the unread set
 *  (`loadUnreadNotifications`), so opening the inbox never reaches for a count. */
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

/** Load the next page of notifications (infinite scroll). */
export async function loadMoreNotifications(): Promise<void> {
  if (notificationsLoadingMore.value || !notificationsHasMore.value) return;

  notificationsLoadingMore.value = true;
  try {
    // Re-cursor and retry when the list moves under the request, rather than
    // appending or dropping. `handleNotificationSSE` re-runs `loadNotifications`
    // on every arrival, under exactly the conditions infinite scroll runs in,
    // and that reload truncates back to one page.
    //
    // Both simpler answers are wrong. Writing the pre-await snapshot back
    // reverts the reload and drops the row it brought in. Appending the page
    // instead spans the truncation, so every row between the reloaded page and
    // this one becomes unreachable by scrolling. Dropping the page wedges the
    // scroll: the sentinel's observer is rebuilt only when `hasMore` changes,
    // and it never left the viewport, so nothing re-asks.
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
      // Moved under us: this page answers a cursor that no longer ends the
      // list, so throw it away and ask again from the new tail.
      if (after.data !== base.data) continue;

      // Dedupe anyway: a reload can have carried some of these rows already.
      const seen = new Set(after.data.map(n => n.id));
      const fresh = (data.notifications || []).filter(n => !seen.has(n.id));
      // The page is only as fresh as the older of the two loads it joins.
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

/** Refresh whichever signal the active notifications tab renders: the bell
 *  badge's unread set (`unreadNotifications`) for the "Unread" tab, the paginated
 *  browse list (`notifications`) for "All". Shared by opening the view (menu
 *  switch) and switching filters so both routes source the tab the same way and
 *  the Unread tab can never fall back to the separately-fetched browse list.
 *  Both loaders self-report failures via Loadable failed / showToast, so the
 *  returned promise only says when the tab's data landed. */
export function refreshActiveNotificationsTab(): Promise<void> {
  return notificationsFilter.value === 'unread' ? loadUnreadNotifications() : loadNotifications();
}

/** Switch between "all" and "unread" filter and refresh the tab's source. The
 *  "Unread" tab renders `unreadNotifications` (the bell badge's single source),
 *  so switching to it refreshes that set in place — badge and list stay one
 *  array. The "All" tab renders the paginated `notifications` browse list. */
export function setNotificationsFilter(filter: NotificationsFilter): void {
  notificationsFilter.value = filter;
  void refreshActiveNotificationsTab();
  void savePreference('notifications_filter', filter);
}

/** Keyed so repeats collapse into one card, and so a landed load can retract
 *  it: the message asserts the count is stale, which a fresh set makes false. */
const UNREAD_STALE_TOAST_KEY = 'refresh-unread-count';

/** Threshold for the unread-load escalation toast. Three consecutive failures
 *  before we bother the user: a single transient failure shouldn't surface, but
 *  a reachable engine that keeps refusing this endpoint should, so the user
 *  knows the badge is stale. Reset on the next success. */
const UNREAD_LOAD_TOAST_THRESHOLD = 3;
const unreadLoadFailures = createFailureCounter(UNREAD_LOAD_TOAST_THRESHOLD, () => {
  showToast(
    // Not "the engine answered nothing": a counted failure is either no answer
    // (the client deadline fired) or a bad one (an HTTP error), and an HTTP
    // error IS an answer. Say what is true of both.
    `Unread count is stale: ${UNREAD_LOAD_TOAST_THRESHOLD} refresh attempts in a row failed`,
    'error',
    { key: UNREAD_STALE_TOAST_KEY },
  );
});

/** Load the unread set — the single source the bell badge derives from. Called
 *  on startup / resume / notification SSE; runs without user intent, so it's
 *  best-effort: individual failures are swallowed (see `.claude/rules/frontend.md`
 *  § "Carve-out: best-effort telemetry") and escalated via a single toast only
 *  after `UNREAD_LOAD_TOAST_THRESHOLD` consecutive failures WHILE THE ENGINE IS
 *  REACHABLE (see the catch below for why an outage is the dot's to report, not
 *  this one's). A landed load retracts the toast. */
export async function loadUnreadNotifications(): Promise<void> {
  const epoch = beginLoad('unread');
  latestUnreadLoad = epoch;
  try {
    const data = await getNotifications({ limit: UNREAD_LOAD_LIMIT, filter: 'unread' })
      .finally(() => loadsInFlight.unread.delete(epoch));
    if (epoch === latestUnreadLoad) {
      batch(() => {
        servedUnreadNotifications.value = { status: 'loaded', data: data.notifications || [] };
        servedNotificationEpochs.value = { ...servedNotificationEpochs.value, unread: epoch };
        pruneReads();
      });
      // Server truth just landed — re-assert the app-icon badge from it, even
      // when the count is unchanged. This is the resume path: a notification
      // read on another device leaves 0 → 0 here, which notifies no subscriber,
      // while the icon still shows what the push wrote.
      syncWorkspaceAppBadge();
    }
    unreadLoadFailures.recordSuccess();
    // The card claims the count is stale. A set just landed, so it isn't.
    // Structural removal (not the user-dismiss path), because this is the
    // toast tracking the signal that drives it, not the user reading it.
    removeToast(UNREAD_STALE_TOAST_KEY);
  } catch (e) {
    // Transient page-lifecycle / reachability noise on an iOS PWA wake over a
    // flaky link (freeze, radio handoff, Tailscale reconnect) surfaces here as
    // either a browser-cancelled AbortError or a transport-layer TypeError
    // (Safari "Load failed") — there's no manual AbortController on this path, so
    // neither carries a definitive "engine is down" signal. Don't count them
    // toward the escalation threshold, or a few background-resume blips falsely
    // trip "Unread count is stale — couldn't reach the engine after 3 tries".
    // The next resume / notification SSE re-syncs the badge, and a genuine
    // sustained outage is owned by the debounced connection dot (connection.ts).
    // A client-side TimeoutError (waited the full 10s window and got nothing) is
    // the stronger "genuinely stuck" signal that still counts. Mirrors the
    // AbortError+transport swallow in `refreshChangesState` / `refreshThreadEvents`.
    if (isAbortError(e) || isTransportError(e)) return;
    // ...but only while the engine is REACHABLE. An unreachable engine is
    // already being reported, once, by the connection dot, and this endpoint
    // failing is then the same fact told twice. It is also the dominant case in
    // practice: over a dropped tunnel the GET hangs rather than refusing, so it
    // dies on the 10s client deadline as a TimeoutError, which is exactly what
    // the line above lets through. The counter therefore means "consecutive
    // failures while the engine was reachable". A disconnected engine neither
    // adds to it nor clears it, so a genuinely broken endpoint is still caught
    // the moment the dot is green.
    if (connectionStatus.value !== 'connected') return;
    unreadLoadFailures.recordFailure();
  } finally {
    pruneReads();
  }
}

/** Handle notification SSE events (NotificationCreated/Read/AllRead). Always
 *  reloads the unread set — the single source the bell badge, the PWA app-icon
 *  badge, AND the "Unread" tab all project from (`unreadNotifications`) — so all
 *  three track the server together. Reloads the paginated "All" browse list only
 *  when it's the visible tab with no detail open: the "Unread" tab needs no
 *  browse reload (it re-renders the just-refreshed unread set), and a browse
 *  reload while a detail is open would drop the currently-viewed row and break
 *  prev/next navigation. */
export function handleNotificationSSE(): void {
  reloadNotificationLists();
  // Under the Tauri desktop app, wake the native unread-indicator loop so the
  // menu-bar tray title (and the dock badge, when a window is open) reflects this
  // change instantly. This handler runs on Created/Read/AllRead, all broadcast
  // AFTER the engine commits, so it's race-free (unlike the optimistic local
  // drop), and it covers reads from another device (their SSE arrives here too).
  // No-op off Tauri; the recompute reads the fresh aggregate.
  if (isTauri()) nudgeDockBadge();
}

/** Reload the unread set, and the "All" browse list when it is the visible
 *  tab with no detail open (see `handleNotificationSSE` for why). */
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

// Deduplication: prevent the same notification from being opened twice in quick
// succession (e.g. SW postMessage + URL-param cold start both fire for one tap).
let _lastViewedId: string | null = null;
let _lastViewedAt = 0;

/** Reset the dedup guard so the same notification can be reopened after closing. */
export function resetViewDedup(): void {
  _lastViewedId = null;
}

/** Mark a notification read on tap. The row leaves the unread set and the
 *  badge at once. It stays out until a load started after the server's
 *  acknowledgement replaces it (ADR 0437). Idempotent, so safe on read rows.
 *  A refused POST shows the row again and says so. */
export function markReadOptimistic(id: string): void {
  const owner = holdReads([id]);
  markNotificationRead(id)
    .then(() => acknowledgeReads([id], owner))
    .catch((error) => {
      abandonReads([id], owner);
      showToast(`Could not mark notification ${id} read: ${errorDetail(error)}`, 'error', {
        key: `notification-read-failed-${id}`,
      });
    });
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
    // Memory first. When either list already holds the row it IS the full
    // notification (see findLoadedNotification), so the detail lands on this
    // tick with no network at all: the inbox row tap, the warm push tap, and
    // the toast tap for a row the page has seen all become instant.
    let notification = findLoadedNotification(id);
    if (!notification) {
      // Genuine miss (the cold push-tap deep link, before the unread set has
      // loaded), so a round-trip is unavoidable. Reveal the pane on the tap and
      // flag the fetch: ContentPane renders the detail's own skeleton, gated
      // past SPINNER_DELAY_MS so a fast open still shows nothing rather than a
      // flash. The overlay stays unwritten until a real notification is in hand,
      // so a failure leaves no phantom nav entry behind.
      notificationDetailPending.value = id;
      revealContentPane();
      notification = await getNotification(id);
    }
    if (!notification) {
      // The tap resolved to nothing: a cold push-tap deep link into a
      // notification the engine no longer has. It answers 200-with-null there,
      // per `getNotification`'s `| null` type. The pane was revealed above, so
      // say why instead of leaving it blank. Reset the dedup stamp, as the catch
      // does, so a re-tap isn't swallowed for the next 10s.
      _lastViewedId = null;
      showToast('Notification not found', 'error');
      return;
    }
    // Open the detail in the content pane (not a modal): set the overlay,
    // reveal the pane, and push a nav entry so panel Back returns to the
    // inbox list and a reload restores the open detail. Mirrors openUrl /
    // openFilePreview.
    openNotificationDetail(notification);
    // Deep-link / push-tap opens land here without the inbox browse list ever
    // having been loaded (the user never opened the Notifications panel), so
    // the detail's prev/next chevrons would sit permanently disabled: they walk
    // `notifications` and resolve currentIndex === -1. Load it now when it
    // doesn't already hold this row. Load BEFORE marking read: the just-pushed
    // row is still unread server-side, so it's returned under either filter
    // ('all' or 'unread'); the subsequent mark-read only flips the loaded row
    // in place (a pending read never removes it), so currentIndex stays
    // valid and the chevrons can walk the inbox. In-app opens from the panel
    // already have the list loaded with this row, so they skip the fetch.
    if (!browseListHas(id)) await loadNotifications();
    markReadOptimistic(id);
  } catch (error) {
    _lastViewedId = null;
    showToast('Failed to load notification: ' + errorDetail(error), 'error');
  } finally {
    // Guarded so a slow first fetch settling after the user has tapped a second
    // notification doesn't clear the second one's skeleton out from under it.
    if (notificationDetailPending.value === id) notificationDetailPending.value = null;
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

/** The keyboard route to the detail's newer/older chevrons (-1 newer, 1 older).
 *  Acts only while the content pane is focused on an open notification, and
 *  returns whether it did. It claims the key at either end of the list too. A
 *  press then never falls through to a transcript the user is not reading. */
export function stepViewedNotification(direction: -1 | 1): boolean {
  const detail = viewingNotification.value;
  if (!detail || focusedPane.value !== 'content') return false;
  void navigateAdjacentNotification(detail.id, direction);
  return true;
}

/** Run when the notification detail panel closes — the overlay is cleared by
 *  panel Back nav, a menu switch, or any restore path, so there is no single
 *  call site to hang this on. Driven by an effect on `viewingNotification` in
 *  store/effects.ts (mirrors the `lastPreviewFile` reset there). Resets the
 *  view-dedup guard so the same notification can be reopened immediately, and
 *  refreshes the "All" browse list when it's the active panel so a now-read row
 *  shows its read state. The "Unread" tab renders `unreadNotifications`, from
 *  which the read row was already dropped optimistically (a pending read), so
 *  it needs no reload. */
export function onNotificationDetailClosed(): void {
  resetViewDedup();
  if (activeMenuItem.value === 'notifications' && notificationsFilter.value === 'all') {
    void loadNotifications();
  }
}

/** Clear every unread row on the tap, then tell the server. On a slow engine
 *  the POST can take tens of seconds, and applying after it made the tap look
 *  dead for that whole wait.
 *
 *  Each cleared row is a pending read, so a load served before the commit
 *  cannot bring it back. Once acknowledged, the lists reload: the server also
 *  marked rows that arrived after the tap, or that a load in flight serves. A failure shows the served rows
 *  again and says so. */
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
