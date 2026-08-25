import { useRef, useEffect } from 'preact/hooks';
import {
  notifications,
  unreadNotifications,
  notificationsFilter,
  notificationsHasMore,
  notificationsLoadingMore,
} from '../../store/store';
import {
  viewNotification,
  markAllRead,
  loadMoreNotifications,
  setNotificationsFilter,
} from '../../store/actions/notifications';
import { dispatchDeepLink } from '../../store/actions/in-app-notification-toast';
import { parseDeepLinkFromInboxRow } from '../../store/actions/notification-deeplink';
import { stripHtml } from '../../utils/escapeHtml';
import { formatTimeAgo, formatNotificationDate } from '../../utils/formatTime';
import { loadedOr, type Loadable, type Notification } from '../../store/types';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { LoadableError } from '../shared/LoadableError';
import { ChevronRightIcon } from '../shared/icons';
import { ListSkeletonOf, useSkeleton, SkText } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';

/** The Loadable the Notifications view renders for a given filter — the single
 *  source of truth that makes the badge and the unread list impossible to
 *  disagree. The "Unread" tab returns the SAME `unreadNotifications` set the bell
 *  badge / PWA app-icon badge / page title project from (store.ts `unreadCount`),
 *  so its length IS the badge count — one array, no second fetch to drift. The
 *  "All" tab returns the paginated `notifications` browse list. Pure + exported
 *  so the invariant is unit-testable (and a future edit can't silently point the
 *  "Unread" tab back at the browse list without failing a test). */
export function notificationsTabSource(
  filter: 'all' | 'unread',
  unread: Loadable<Notification[]>,
  all: Loadable<Notification[]>,
): Loadable<Notification[]> {
  return filter === 'unread' ? unread : all;
}

export function NotificationsView() {
  const sentinelRef = useRef<HTMLDivElement>(null);

  const filter = notificationsFilter.value;
  const loadable = notificationsTabSource(filter, unreadNotifications.value, notifications.value);
  const items = loadedOr(loadable, []);
  // The "All" browse list transitions through 'loading' (loadNotifications). The
  // unread set never does — `loadUnreadNotifications` applies in place so the bell
  // badge never blinks to 0 on a reload — so on the "Unread" tab, treat the
  // first-load 'not-loaded' state as loading too. Delay-gated either way, so a
  // fast load shows nothing rather than a flash; a slow first load shows the
  // skeleton instead of a blank panel.
  const showLoading = useDelayedFlag(
    loadable.status === 'loading' || (filter === 'unread' && loadable.status === 'not-loaded'),
  );
  // Pagination applies to the "All" browse list only — the unread set is loaded
  // whole (bounded by UNREAD_LOAD_LIMIT; a larger backlog already renders as
  // "99+" on the badge), so there is no "load more" on the "Unread" tab.
  const hasMore = filter === 'all' && notificationsHasMore.value;
  const loadingMore = notificationsLoadingMore.value;

  // Infinite scroll: observe a sentinel at the bottom of the list. The real
  // scroll container is the ancestor `.content-pane-body` (overflow-y: auto in
  // panels/shell.css), NOT this view's `.panel-content` — a scroll listener on
  // `.panel-content` never fired because that element doesn't scroll (scroll
  // events don't bubble). Rooting the observer at `.content-pane-body` (mirrors
  // ThreadDrawer's pattern) loads the next page as the sentinel comes into
  // view. `loadMoreNotifications` self-guards against concurrent calls and the
  // no-more-pages case, so a stray intersection is harmless.
  useEffect(() => {
    const sentinel = sentinelRef.current;
    if (!sentinel || !hasMore) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting) void loadMoreNotifications();
      },
      { root: sentinel.closest('.content-pane-body'), threshold: 0 },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [hasMore]);

  const emptyMessage =
    filter === 'unread' ? 'No unread notifications' : 'No notifications';

  return (
    <div class="panel-content">
      <div class="dropdown-panel-toolbar">
        <div class="notifications-filter segmented-control">
          <button
            class={`segmented-btn ${filter === 'all' ? 'active' : ''}`}
            onClick={() => setNotificationsFilter('all')}
          >
            All
          </button>
          <button
            class={`segmented-btn ${filter === 'unread' ? 'active' : ''}`}
            onClick={() => setNotificationsFilter('unread')}
          >
            Unread
          </button>
        </div>
        <button
          class="mark-all-read"
          onClick={() => void markAllRead()}
          disabled={items.length === 0 || items.every((n) => n.read)}
        >
          Mark all read
        </button>
      </div>
      {loadable.status === 'failed' ? (
        <LoadableError noun="notifications" error={loadable.error} />
      ) : (
        <LoadingFade
          showSkeleton={showLoading}
          skeleton={<ListSkeletonOf fill row={() => <NotificationRow />} />}
        >
          {loadable.status === 'loaded' ? (
            items.length === 0 ? (
              <div class="empty-state">{emptyMessage}</div>
            ) : (
              <>
                {items.map((n) => (
                  <NotificationRow key={n.id} n={n} />
                ))}
                {hasMore && (
                  <div
                    ref={sentinelRef}
                    class="dropdown-panel-loading-more"
                    style={loadingMore ? undefined : 'opacity: 0.4'}
                  >
                    {loadingMore ? 'Loading more...' : 'Scroll for more'}
                  </div>
                )}
              </>
            )
          ) : null}
        </LoadingFade>
      )}
    </div>
  );
}

/** Self-skeletonizing notification row: rendered with no props inside a
 *  SkeletonProvider (`<NotificationRow />`) it draws itself as a loading
 *  placeholder via the Sk* leaves; with a real `n` it renders normally. The
 *  prop is optional only to support the skeleton call; the real call site
 *  passes it. In skeleton mode it renders a non-interactive `<div>` (no click
 *  handler), mirroring the three lines (title, summary, time).
 *
 *  The row carries NO icon. It wore a 📋 clipboard until the emoji sweep, which
 *  was the last colour emoji in this pane and said nothing: every row in the
 *  Notifications list is a notification, so a per-row glyph only restated the
 *  pane it was already inside. Unread state is carried by `.notification-row.unread`.
 *  Its `<SkBlock>` stand-in went with it, since a skeleton mirrors the real row
 *  by construction and a shimmer dot standing in for nothing is a hole.
 *
 *  The row body dispatches the notification's own `tap`, so it lands where the
 *  toast and the OS push land. A `navigate` tap therefore leaves the message
 *  itself unseen, and that row grows a trailing chevron for the detail. A
 *  modal-tap row's body already opens the detail, so it gets no chevron. The
 *  chevron is a SIBLING of the row button, not a child: a button cannot contain
 *  a button, and a nested one would fire both handlers on one tap. It wears the
 *  shared `row-icon` band, whose 2.25rem tap target is what a row icon needs to
 *  be hittable on a phone. */
export function NotificationRow({ n }: { n?: Notification }) {
  const sk = useSkeleton();
  const date = n ? new Date(n.created_at) : null;
  const timeAgo = date ? formatTimeAgo(date) : '';
  const dateStr = date ? formatNotificationDate(date) : '';
  const jumps = !sk && n?.tap?.kind === 'navigate';
  const rowClass = `notification-row ${sk || n?.read ? '' : 'unread'}`;
  const body = (
    <>
      <div class="title notification-title">
        <SkText w="9rem">{n?.title}</SkText>
      </div>
      <SkText class="notification-summary" as="div" w="16rem">
        {n ? stripHtml(renderMarkdown(n.message)) : ''}
      </SkText>
      <SkText class="notification-time" as="div" w="6rem">
        {timeAgo} · {dateStr}
      </SkText>
    </>
  );
  if (sk) {
    return (
      <div class={rowClass}>
        <div class="notification-item">{body}</div>
      </div>
    );
  }
  return (
    <div class={rowClass}>
      <button
        class="notification-item"
        onClick={() => n && dispatchDeepLink(parseDeepLinkFromInboxRow(n))}
      >
        {body}
      </button>
      {jumps && n && (
        <button
          class="icon-btn row-icon notification-row-detail-btn"
          onClick={() => void viewNotification(n.id)}
          aria-label="Open notification"
          data-tooltip="Open notification"
        >
          <ChevronRightIcon />
        </button>
      )}
    </div>
  );
}
