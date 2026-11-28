import { dispatchDeepLink } from '../../store/actions/in-app-notification-toast';
import { parseDeepLinkFromInboxRow } from '../../store/actions/notification-deeplink';
import {
  notifications,
  notificationsHasMore,
  viewingNotification,
  appsList,
} from '../../store/store';
import { openApp } from '../../store/actions/apps';
import { navigateToTrigger } from '../../store/actions/triggers';
import { navigateAdjacentNotification } from '../../store/actions/notifications';
import { discussNotification } from '../../store/actions/notification-discuss';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { handleNavigationRequest } from '../../store/actions/navigation-request';
import { composeHandlers } from '../chat/composeHandlers';
import { handleMarkdownLinkClick } from '../shared/markdownLinkClick';
import { formatNotificationDate } from '../../utils/formatTime';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { linkifyPaths } from '../../utils/linkifyPaths';
import { loadedOr } from '../../store/types';
import { ChevronUpIcon, ChevronDownIcon } from '../shared/icons';
import { useSkeleton, SkText } from '../shared/Skeleton';
import { resolveLinkedApp } from './resolveLinkedApp';
import { appById } from '../../store/appsById';
import { notificationPosition } from './notificationPosition';
import { navigateTapLabel } from './notificationTapLabel';
import { notificationActions, notificationTriggerId } from './notificationActions';

export function NotificationDetailInline() {
  const sk = useSkeleton();
  const detail = viewingNotification.value;
  if (sk) return <NotificationDetailSkeleton />;
  if (!detail) return null;

  const position = notificationPosition(
    loadedOr(notifications.value, []),
    detail.id,
    notificationsHasMore.value,
  );

  const linked = resolveLinkedApp(detail.app_id, appsList.value, appById(detail.app_id ?? ''));
  const apps = loadedOr(appsList.value, []);
  const content = linkifyPaths(renderMarkdown(detail.message), [], apps);
  const dateStr = formatNotificationDate(new Date(detail.created_at));

  const actions = notificationActions(detail, linked.kind === 'linked');
  const triggerId = notificationTriggerId(detail);

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

  const discussHandlers = composeHandlers(
    () => { void discussNotification(detail!); },
    () => {},
  );

  function handleNavigateTap() {
    if (!actions.navTap) return;
    void handleNavigationRequest(actions.navTap);
  }

  function handleOpenTrigger() {
    if (!triggerId) return;
    void navigateToTrigger(triggerId, 'a notification');
  }

  function handleBodyClick(e: MouseEvent) {
    handleMarkdownLinkClick(e, apps, 'a notification');
  }

  return (
    <div class="notification-detail">
      <div class="notification-detail-header">
        <span class="notification-detail-date">{dateStr}</span>
        <button
          class="notification-detail-nav newer"
          onClick={() => void navigateAdjacentNotification(detail.id, -1)}
          disabled={!position.hasNewer}
          aria-label="Newer notification"
          data-tooltip="Newer notification"
        >
          <ChevronUpIcon />
        </button>
        <button
          class="notification-detail-nav older"
          onClick={() => void navigateAdjacentNotification(detail.id, 1)}
          disabled={!position.hasOlder}
          aria-label="Older notification"
          data-tooltip="Older notification"
        >
          <ChevronDownIcon />
        </button>
      </div>
      <h2 class="notification-detail-title">{detail.title || 'Notification'}</h2>
      <div
        class="notification-detail-body markdown-content authored-html"
        onClick={handleBodyClick}
        dangerouslySetInnerHTML={{ __html: content }}
      />
      <div class="notification-detail-actions">
        {linked.kind === 'linked' && (
          <button class="action-btn" onClick={handleOpenApp}>
            Open {linked.app.name}
          </button>
        )}
        {actions.openThread && (
          <button class="action-btn" onClick={handleOpenThread}>
            Open thread
          </button>
        )}
        {actions.discuss && (
          <button class="action-btn" {...discussHandlers}>
            Discuss
          </button>
        )}
        {actions.openTrigger && (
          <button class="action-btn" onClick={handleOpenTrigger}>
            Open trigger
          </button>
        )}
        {detail.tap?.kind === 'restore-snapshot' && (
          <button class="action-btn" onClick={() => dispatchDeepLink(parseDeepLinkFromInboxRow(detail))}>
            Undo restore
          </button>
        )}
        {detail.tap?.kind === 'undo-config-change' && (
          <button class="action-btn" onClick={() => dispatchDeepLink(parseDeepLinkFromInboxRow(detail))}>
            Undo
          </button>
        )}
        {actions.navTap && (
          <button class="action-btn" onClick={handleNavigateTap}>
            {navigateTapLabel(actions.navTap)}
          </button>
        )}
      </div>
      {linked.kind === 'unknown' && (
        <div class="notification-detail-actions">
          <span class="error-text">Unknown app: {linked.appId}</span>
        </div>
      )}
    </div>
  );
}

function NotificationDetailSkeleton() {
  return (
    <div class="notification-detail">
      <div class="notification-detail-header">
        <span class="notification-detail-date">
          <SkText w="7rem" />
        </span>
      </div>
      <h2 class="notification-detail-title">
        <SkText w="12rem" />
      </h2>
      <div class="notification-detail-body markdown-content">
        <SkText as="div" w="100%" />
        <SkText as="div" w="92%" />
        <SkText as="div" w="64%" />
      </div>
    </div>
  );
}
