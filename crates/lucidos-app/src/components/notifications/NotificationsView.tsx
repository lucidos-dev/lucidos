import { useRef, useEffect } from 'preact/hooks';
import {
  notifications,
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
import { stripHtml } from '../../utils/escapeHtml';
import { formatTimeAgo, formatNotificationDate } from '../../utils/formatTime';
import { loadedOr, type Notification } from '../../store/types';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf, useSkeleton, SkText, SkBlock } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';

export function NotificationsView() {
  const sentinelRef = useRef<HTMLDivElement>(null);

  const loadable = notifications.value;
  const items = loadedOr(loadable, []);
  const showLoading = useDelayedLoading(loadable);
  const filter = notificationsFilter.value;
  const hasMore = notificationsHasMore.value;
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
 *  handler), mirroring the three lines (emoji + title, summary, time). */
function NotificationRow({ n }: { n?: Notification }) {
  const sk = useSkeleton();
  const date = n ? new Date(n.created_at) : null;
  const timeAgo = date ? formatTimeAgo(date) : '';
  const dateStr = date ? formatNotificationDate(date) : '';
  const className = `notification-item ${sk || n?.read ? '' : 'unread'}`;
  const body = (
    <>
      <div class="title notification-title">
        <SkBlock w="1rem" h="1rem" circle>
          <span class="trigger-icon">📋</span>
        </SkBlock>
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
  if (sk) return <div class={className}>{body}</div>;
  return (
    <button class={className} onClick={() => n && void viewNotification(n.id)}>
      {body}
    </button>
  );
}
