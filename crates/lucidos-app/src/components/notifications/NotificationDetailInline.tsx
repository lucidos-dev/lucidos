import {
  notifications,
  notificationsHasMore,
  viewingNotification,
  appsList,
  showToast,
} from '../../store/store';
import { openApp, openAppById } from '../../store/actions/apps';
import { navigateToTrigger } from '../../store/actions/triggers';
import { navigateAdjacentNotification } from '../../store/actions/notifications';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { handleNavigationRequest } from '../../store/actions/navigation-request';
import { formatNotificationDate } from '../../utils/formatTime';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { linkifyPaths } from '../../utils/linkifyPaths';
import { loadedOr } from '../../store/types';
import { ChevronLeftIcon, ChevronRightIcon } from '../shared/icons';
import { useSkeleton, SkText } from '../shared/Skeleton';
import { resolveLinkedApp } from './resolveLinkedApp';
import { navigateTapLabel } from './notificationTapLabel';
import { notificationActions, notificationTriggerId } from './notificationActions';

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
  const sk = useSkeleton();
  const detail = viewingNotification.value;
  // Skeleton mode renders the same frame with shimmer leaves, so the detail's
  // loading placeholder is this component and cannot drift from it (the
  // self-skeletonizing rule in `.claude/rules/frontend.md`). Only reachable via
  // `ContentPane`'s pending branch, which mounts it inside a SkeletonProvider
  // while a notification the page does not already hold is being fetched.
  if (sk) return <NotificationDetailSkeleton />;
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

  // Which buttons the actions row offers, including the dedup between a
  // `navigate` tap and a dedicated button for the same destination. Pure and
  // unit-tested in notificationActions.test.ts.
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

  function handleNavigateTap() {
    if (!actions.navTap) return;
    handleNavigationRequest(actions.navTap);
  }

  function handleOpenTrigger() {
    if (!triggerId) return;
    // navigateToTrigger re-fetches the trigger list on a cache miss before
    // concluding the trigger is gone, and names this notification as the origin
    // in that toast. Mirrors handleBodyClick's openAppById call.
    void navigateToTrigger(triggerId, 'a notification');
  }

  function handleBodyClick(e: MouseEvent) {
    const target = e.target as HTMLElement;
    // A `[name](trigger:<id>)` the agent wrote into the notification body. Same
    // destination as the Open trigger button above, so the same call.
    const triggerLink = target.closest<HTMLAnchorElement>('a.trigger-link');
    if (triggerLink) {
      e.preventDefault();
      const linkedTriggerId = triggerLink.dataset.triggerId;
      if (!linkedTriggerId) {
        showToast('Cannot open trigger link: the link is missing its trigger id', 'error');
        return;
      }
      void navigateToTrigger(linkedTriggerId, 'a notification');
      return;
    }
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
      {actions.any && (
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
          {actions.openTrigger && (
            <button class="action-btn" onClick={handleOpenTrigger}>
              Open trigger
            </button>
          )}
          {actions.navTap && (
            <button class="action-btn" onClick={handleNavigateTap}>
              {navigateTapLabel(actions.navTap)}
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

/** The detail drawn as a loading placeholder: the same frame (nav row, title,
 *  body) with shimmer leaves instead of content. Mirrors the real layout above
 *  by sharing its class names, so the swap to real content doesn't reflow.
 *
 *  Rendered while a notification is being FETCHED, which after the memory-first
 *  open in `viewNotification` means only the cold push-tap deep link: the page
 *  holds neither list yet, so it genuinely has to ask the engine. The chevrons
 *  and action buttons are omitted rather than shimmered, because which of them
 *  exist is a property of the row we haven't got. */
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
