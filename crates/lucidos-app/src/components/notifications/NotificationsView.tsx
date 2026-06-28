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
import { loadedOr } from '../../store/types';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeleton } from '../shared/ListSkeleton';
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
        <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeleton />}>
          {loadable.status === 'loaded' ? (
            items.length === 0 ? (
              <div class="empty-state">{emptyMessage}</div>
            ) : (
              <>
                {items.map((n) => {
                  const date = new Date(n.created_at);
                  const timeAgo = formatTimeAgo(date);
                  const dateStr = formatNotificationDate(date);
                  return (
                    <button
                      key={n.id}
                      class={`notification-item ${n.read ? '' : 'unread'}`}
                      onClick={() => void viewNotification(n.id)}
                    >
                      <div class="title notification-title">
                        <span class="trigger-icon">📋</span>
                        {n.title}
                      </div>
                      <div class="notification-summary">
                        {stripHtml(renderMarkdown(n.message))}
                      </div>
                      <div class="notification-time">
                        {timeAgo} · {dateStr}
                      </div>
                    </button>
                  );
                })}
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
