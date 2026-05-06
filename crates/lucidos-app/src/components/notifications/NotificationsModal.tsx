import { useCallback } from 'preact/hooks';
import {
  notifications,
  unreadCount,
  notificationsModalOpen,
  notificationModalDetail,
  appsList,
  showToast,
} from '../../store/store';
import {
  getNotification,
  markNotificationRead,
} from '../../api/client';
import { openApp } from '../../store/actions/apps';
import { resetViewDedup, loadNotifications } from '../../store/actions/notifications';
import { formatNotificationDate } from '../../utils/formatTime';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { linkifyPaths } from '../../utils/linkifyPaths';
import { loadedOr } from '../../store/types';
import { ModalOverlay } from '../shared/ModalOverlay';
import { ChevronLeftIcon, ChevronRightIcon, CloseIcon } from '../shared/icons';
import { errorDetail } from '../../utils/errorDetail';
import { resolveLinkedApp } from './resolveLinkedApp';

export function NotificationsModal() {
  const isOpen = notificationsModalOpen.value;
  const detail = notificationModalDetail.value;

  const close = useCallback(() => {
    notificationsModalOpen.value = false;
    notificationModalDetail.value = null;
    resetViewDedup();
    loadNotifications();
  }, []);

  if (!isOpen || !detail) return null;

  const items = loadedOr(notifications.value, []);
  const currentIndex = items.findIndex((n) => n.id === detail.id);
  const hasPrev = currentIndex > 0;
  const hasNext = currentIndex >= 0 && currentIndex < items.length - 1;

  async function navigate(index: number) {
    const target = items[index];
    if (!target) return;
    try {
      const [, full] = await Promise.all([
        markNotificationRead(target.id),
        getNotification(target.id),
      ]);
      if (full) {
        notificationModalDetail.value = full;
        const current = notifications.value;
        if (current.status === 'loaded') {
          notifications.value = {
            status: 'loaded',
            data: current.data.map((n) => n.id === target.id ? { ...n, read: true } : n),
          };
        }
        if (!target.read && unreadCount.value > 0) {
          unreadCount.value = unreadCount.value - 1;
        }
      }
    } catch (error) {
      showToast('Failed to load notification: ' + errorDetail(error), 'error');
    }
  }

  const linked = resolveLinkedApp(detail.app_id, appsList.value);
  const apps = loadedOr(appsList.value, []);
  const content = linkifyPaths(renderMarkdown(detail.message), [], apps);
  const dateStr = formatNotificationDate(new Date(detail.created_at));

  function handleOpenApp() {
    if (linked.kind !== 'linked') return;
    close();
    openApp(linked.app);
  }

  function handleBodyClick(e: MouseEvent) {
    const target = e.target as HTMLElement;
    const link = target.closest<HTMLAnchorElement>('a.app-link');
    if (!link) return;
    e.preventDefault();
    const appId = link.dataset.appId;
    const app = apps.find((s) => s.id === appId);
    if (app) {
      close();
      openApp(app);
    } else {
      showToast(`Unknown app: ${appId ?? '(missing id)'}`, 'error');
    }
  }

  return (
    <ModalOverlay onClose={close} class="notifications-modal-overlay">
      <div class="notifications-modal" onClick={(e) => e.stopPropagation()}>
        <div class="notifications-modal-header">
          <button
            class="icon-btn"
            onClick={() => navigate(currentIndex - 1)}
            disabled={!hasPrev}
            aria-label="Previous notification"
          >
            <ChevronLeftIcon />
          </button>
          <button
            class="icon-btn"
            onClick={() => navigate(currentIndex + 1)}
            disabled={!hasNext}
            aria-label="Next notification"
          >
            <ChevronRightIcon />
          </button>
          <span class="notifications-modal-title">
            <span class="trigger-icon">{linked.kind === 'linked' && linked.app.icon ? linked.app.icon : '\u{1F4CB}'}</span>
            {linked.kind === 'linked' ? (
              <a class="accent-link" onClick={handleOpenApp}>{detail.title}</a>
            ) : (
              detail.title
            )}
          </span>
          <button class="icon-btn" onClick={close} aria-label="Close">
            <CloseIcon />
          </button>
        </div>
        <div class="notifications-modal-detail-date">{dateStr}</div>
        <div
          class="notification-detail-body markdown-content"
          onClick={handleBodyClick}
          dangerouslySetInnerHTML={{ __html: content }}
        />
        {linked.kind === 'linked' && (
          <div class="notification-detail-actions">
            <button class="action-btn" onClick={handleOpenApp}>
              Open {linked.app.name}
            </button>
          </div>
        )}
        {linked.kind === 'unknown' && (
          <div class="notification-detail-actions">
            <span class="error-text">Unknown app: {linked.appId}</span>
          </div>
        )}
      </div>
    </ModalOverlay>
  );
}
