import { useRef, useCallback } from 'preact/hooks';
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

export function NotificationsView() {
  const listRef = useRef<HTMLDivElement>(null);

  const handleScroll = useCallback(() => {
    const el = listRef.current;
    if (!el) return;
    if (el.scrollTop + el.clientHeight >= el.scrollHeight - 50) {
      loadMoreNotifications();
    }
  }, []);

  const loadable = notifications.value;
  const items = loadedOr(loadable, []);
  const showLoading = useDelayedLoading(loadable);
  const filter = notificationsFilter.value;
  const hasMore = notificationsHasMore.value;
  const loadingMore = notificationsLoadingMore.value;

  const emptyMessage =
    filter === 'unread' ? 'No unread notifications' : 'No notifications';

  return (
    <div class="panel-content" ref={listRef} onScroll={handleScroll}>
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
          onClick={markAllRead}
          disabled={items.length === 0 || items.every((n) => n.read)}
        >
          Mark all read
        </button>
      </div>
      {loadable.status === 'failed' ? (
        <div class="empty-state error-text">Failed to load notifications: {loadable.error}</div>
      ) : loadable.status !== 'loaded' ? (
        showLoading ? <div class="loading-spinner" /> : null
      ) : items.length === 0 ? (
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
                onClick={() => viewNotification(n.id)}
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
          {loadingMore && (
            <div class="dropdown-panel-loading-more">Loading more...</div>
          )}
          {!loadingMore && hasMore && (
            <div class="dropdown-panel-loading-more" style="opacity: 0.4">
              Scroll for more
            </div>
          )}
        </>
      )}
    </div>
  );
}
