import { useRef, useEffect } from 'preact/hooks';
import {
  notifications,
  unreadNotifications,
  notificationsFilter,
  notificationsHasMore,
  notificationsLoadingMore,
  type NotificationsFilter,
} from '../../store/store';
import {
  viewNotification,
  markAllRead,
  loadMoreNotifications,
  setNotificationsFilter,
  refreshActiveNotificationsTab,
} from '../../store/actions/notifications';
import { dispatchDeepLink } from '../../store/actions/in-app-notification-toast';
import { parseDeepLinkFromInboxRow } from '../../store/actions/notification-deeplink';
import { stripHtml } from '../../utils/escapeHtml';
import { formatTimeAgo, formatNotificationDate } from '../../utils/formatTime';
import { loadedOr, type Loadable, type Notification } from '../../store/types';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { LoadableError } from '../shared/LoadableError';
import { ChevronRightIcon } from '../shared/icons';
import { ListSkeletonOf, useSkeleton, SkText } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';

export function notificationsTabSource(
  filter: NotificationsFilter,
  unread: Loadable<Notification[]>,
  all: Loadable<Notification[]>,
): Loadable<Notification[]> {
  return filter === 'unread' ? unread : all;
}

export function notificationsTabIsLoading(loadable: Loadable<Notification[]>): boolean {
  return loadable.status === 'loading' || loadable.status === 'not-loaded';
}

export function NotificationsView() {
  const sentinelRef = useRef<HTMLDivElement>(null);
  usePanelRefresh('notifications', refreshActiveNotificationsTab);

  const filter = notificationsFilter.value;
  const loadable = notificationsTabSource(filter, unreadNotifications.value, notifications.value);
  const items = loadedOr(loadable, []);
  const showLoading = useDelayedFlag(notificationsTabIsLoading(loadable));
  const hasMore = filter === 'all' && notificationsHasMore.value;
  const loadingMore = notificationsLoadingMore.value;
  const showLoadingMore = useDelayedFlag(loadingMore);

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
                <LoadingFade showSkeleton={showLoadingMore} skeleton={<ListSkeletonOf count={2} row={() => <NotificationRow />} />}>
                  {null}
                </LoadingFade>
                {hasMore && (
                  <div ref={sentinelRef} class="dropdown-panel-loading-more" style="opacity: 0.4">
                    {!loadingMore && 'Scroll for more'}
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
