import { useCallback } from 'preact/hooks';
import {
  notifications,
  notificationsModalOpen,
  notificationModalDetail,
  appsList,
  showToast,
} from '../../store/store';
import { openApp } from '../../store/actions/apps';
import {
  closeNotificationsModal,
  navigateToNotification,
} from '../../store/actions/notifications';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { handleNavigationRequest } from '../../store/actions/navigation-request';
import { formatNotificationDate } from '../../utils/formatTime';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { linkifyPaths } from '../../utils/linkifyPaths';
import { loadedOr } from '../../store/types';
import { Overlay } from '../shared/Overlay';
import { ChevronLeftIcon, ChevronRightIcon, CloseIcon } from '../shared/icons';
import { resolveLinkedApp } from './resolveLinkedApp';
import { navigateTapLabel } from './notificationTapLabel';

export function NotificationsModal() {
  const isOpen = notificationsModalOpen.value;
  const detail = notificationModalDetail.value;

  const close = useCallback(() => {
    closeNotificationsModal();
  }, []);

  if (!isOpen || !detail) return null;

  const items = loadedOr(notifications.value, []);
  const currentIndex = items.findIndex((n) => n.id === detail.id);
  const hasPrev = currentIndex > 0;
  const hasNext = currentIndex >= 0 && currentIndex < items.length - 1;

  function navigate(index: number) {
    const target = items[index];
    if (!target) return;
    void navigateToNotification(target.id);
  }

  const linked = resolveLinkedApp(detail.app_id, appsList.value);
  const apps = loadedOr(appsList.value, []);
  const content = linkifyPaths(renderMarkdown(detail.message), [], apps);
  const dateStr = formatNotificationDate(new Date(detail.created_at));

  // A `navigate` tap (e.g. the "N changes ready to apply" trigger push → the
  // Changes panel) is actionable from the OS-push tap and the in-app toast, but
  // the inbox modal would otherwise ignore the tap entirely. Surface it as a
  // button here too. Skip it when a dedicated button already covers the same
  // destination (thread / app), so we never show two buttons that do the same.
  const navTap = detail.tap?.kind === 'navigate' ? detail.tap.to : null;
  const navDuplicatesDedicatedButton =
    (navTap?.target === 'thread' && !!detail.thread_id) ||
    (navTap?.target === 'app' && linked.kind === 'linked');
  const showNavButton = !!navTap && !navDuplicatesDedicatedButton;

  function handleOpenApp() {
    if (linked.kind !== 'linked') return;
    close();
    openApp(linked.app);
  }

  function handleOpenThread() {
    if (!detail?.thread_id) return;
    close();
    focusThreadOrBootstrap(detail.thread_id, {
      targetEventId: detail.event_id ?? null,
    });
  }

  function handleNavigateTap() {
    if (!navTap) return;
    close();
    handleNavigationRequest(navTap);
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
    <Overlay open onClose={close} overlayClass="notifications-modal-overlay" panelClass="notifications-modal">
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
              <button type="button" class="accent-link" onClick={handleOpenApp}>{detail.title}</button>
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
        {(linked.kind === 'linked' || detail.thread_id || showNavButton) && (
          <div class="notification-detail-actions">
            {linked.kind === 'linked' && (
              <button class="action-btn" onClick={handleOpenApp}>
                Open {linked.app.name}
              </button>
            )}
            {detail.thread_id && (
              <button class="action-btn" onClick={handleOpenThread}>
                Open thread
              </button>
            )}
            {showNavButton && navTap && (
              <button class="action-btn" onClick={handleNavigateTap}>
                {navigateTapLabel(navTap)}
              </button>
            )}
          </div>
        )}
        {linked.kind === 'unknown' && (
          <div class="notification-detail-actions">
            <span class="error-text">Unknown app: {linked.appId}</span>
          </div>
        )}
    </Overlay>
  );
}
