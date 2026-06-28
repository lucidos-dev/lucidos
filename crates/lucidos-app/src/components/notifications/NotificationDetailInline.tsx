import {
  notifications,
  notificationsHasMore,
  viewingNotification,
  appsList,
  showToast,
} from '../../store/store';
import { openApp, openAppById } from '../../store/actions/apps';
import { navigateAdjacentNotification } from '../../store/actions/notifications';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { handleNavigationRequest } from '../../store/actions/navigation-request';
import { formatNotificationDate } from '../../utils/formatTime';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { linkifyPaths } from '../../utils/linkifyPaths';
import { loadedOr } from '../../store/types';
import { ChevronLeftIcon, ChevronRightIcon } from '../shared/icons';
import { resolveLinkedApp } from './resolveLinkedApp';
import { navigateTapLabel } from './notificationTapLabel';

/** The notification detail rendered directly in the content pane (replacing the
 *  former modal). The content-pane header owns the title and the back/forward
 *  nav; this body renders the date, the markdown body, the action buttons, and
 *  the floating prev/next chevrons that walk the inbox list (side-centered,
 *  styled like the thread-view scroll chevrons).
 *
 *  Source-level contract: this component must not write store signals directly —
 *  every mutation routes through actions/notifications.ts (prev/next) or the
 *  navigation actions (open app / thread / nav-tap). */
export function NotificationDetailInline() {
  const detail = viewingNotification.value;
  if (!detail) return null;

  const items = loadedOr(notifications.value, []);
  const currentIndex = items.findIndex((n) => n.id === detail.id);
  const hasPrev = currentIndex > 0;
  // The "next" (older) chevron stays visible at the last loaded item when the
  // server has more pages — the tap loads the next page before stepping, so
  // navigation walks the whole inbox, not just the first loaded page.
  const hasNext =
    currentIndex >= 0 &&
    (currentIndex < items.length - 1 || notificationsHasMore.value);

  const linked = resolveLinkedApp(detail.app_id, appsList.value);
  const apps = loadedOr(appsList.value, []);
  const content = linkifyPaths(renderMarkdown(detail.message), [], apps);
  const dateStr = formatNotificationDate(new Date(detail.created_at));

  // A `navigate` tap (e.g. the "N changes ready to apply" trigger push → the
  // Changes panel) is actionable from the OS-push tap and the in-app toast, but
  // the inbox detail would otherwise ignore the tap entirely. Surface it as a
  // button here too. Skip it when a dedicated button already covers the same
  // destination (thread / app), so we never show two buttons that do the same.
  const navTap = detail.tap?.kind === 'navigate' ? detail.tap.to : null;
  const navDuplicatesDedicatedButton =
    (navTap?.target === 'thread' && !!detail.thread_id) ||
    (navTap?.target === 'app' && linked.kind === 'linked');
  const showNavButton = !!navTap && !navDuplicatesDedicatedButton;

  function handleOpenApp() {
    if (linked.kind !== 'linked') return;
    openApp(linked.app);
  }

  function handleOpenThread() {
    if (!detail?.thread_id) return;
    focusThreadOrBootstrap(detail.thread_id, {
      targetEventId: detail.event_id ?? null,
    });
  }

  function handleNavigateTap() {
    if (!navTap) return;
    handleNavigationRequest(navTap);
  }

  function handleBodyClick(e: MouseEvent) {
    const target = e.target as HTMLElement;
    const link = target.closest<HTMLAnchorElement>('a.app-link');
    if (!link) return;
    e.preventDefault();
    const appId = link.dataset.appId;
    if (!appId) {
      showToast('Cannot open app link: the link is missing its app id', 'error');
      return;
    }
    // Route through openAppById, NOT a `apps.find(...)` on the cached list:
    // openAppById re-scans disk on a cache miss before concluding the app is
    // gone, so a link to an app written by a hint-less channel (run_bash /
    // run_python) — which the cached appsList may lag on — still opens instead
    // of falsely toasting "Unknown app". It also names the id + origin on a
    // genuine miss.
    void openAppById(appId, 'a notification');
  }

  return (
    <div class="notification-detail">
      {/* Nav row above the title: prev/next chevrons bracketing the date. Both
       *  chevrons are always rendered, sitting `disabled` (greyed, no pointer
       *  events) at the first / last notification rather than vanishing — the
       *  prev/next affordance stays visible even when not currently available,
       *  and the fixed slot keeps the date centered between them. */}
      <div class="notification-detail-header">
        <button
          class="notification-detail-nav prev"
          onClick={() => void navigateAdjacentNotification(detail.id, -1)}
          disabled={!hasPrev}
          aria-label="Previous notification"
          data-tooltip="Previous notification"
        >
          <ChevronLeftIcon />
        </button>
        <span class="notification-detail-date">{dateStr}</span>
        <button
          class="notification-detail-nav next"
          onClick={() => void navigateAdjacentNotification(detail.id, 1)}
          disabled={!hasNext}
          aria-label="Next notification"
          data-tooltip="Next notification"
        >
          <ChevronRightIcon />
        </button>
      </div>
      <h2 class="notification-detail-title">{detail.title || 'Notification'}</h2>
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
    </div>
  );
}
