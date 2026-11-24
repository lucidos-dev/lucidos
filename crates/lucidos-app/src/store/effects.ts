import { removeWindowItem, writeWindowItem } from '../utils/windowStorage';
import { effect, untracked } from '@preact/signals';
import { pageTitle, visibleWorkspaceName, animationSpeed, durationScale, stepsExpanded, detailsExpanded, expandedFolders, threadDrawerOpen, selectedScope, notificationsFilter, NOTIFICATIONS_FILTER_STORAGE_KEY, collapsedExchanges, collapsedInitiators, openSubAgentGroups, OPEN_SUB_AGENT_GROUPS_KEY, filePreviewSource, filePreviewWrap, diffWholeFile, diffSideBySide, filePreviewEditing, previewFile, viewingNotification, repoSelectedChangeId, inputMode, showToast, removeToast, engineRestarting, SELECTED_CHANGE_KEY, STEPS_EXPANDED_KEY, DETAILS_EXPANDED_KEY, persistTurnControl } from './store';
import { clientRefreshing } from '../hooks/sw-update';
import { handleRestartTimeout } from './actions/connection';
import { onNotificationDetailClosed } from './actions/notifications';
import { installAppAppearanceSync } from './actions/app-appearance';
import { installAppFullscreenWindowSync } from './actions/apps';
import { installNotificationToastLifetime } from './actions/in-app-notification-toast';
import { installUnregisteredRepoTargetReset } from './actions/compose';
import { installLiveUtteranceRow } from './liveUtterance';
import { syncWorkspaceAppBadge } from './actions/app-badge';
import { pushNativeWindowTitle } from '../utils/windowTitle';
import { installMotionAttribute } from '../utils/motion';
import { installThemeEffectsAttribute } from '../utils/themeEffects';
import { ANIMATION_SPEED_STORAGE_KEY } from '@lucidos/appearance';
import { drawerGrouping, appsList } from './store';
import { threadFilterPanelOpen, closeThreadFilterPanel } from './threadFilterPanel';
import { appIdsMissingFromFacets } from './appFilters';
import { reloadFacetsForApps } from './actions/thread-loading';
import { appById } from './appsById';
import { forgetAppById, readAppsById } from './actions/appsById';

effect(() => {
  document.title = pageTitle.value;
});

effect(() => {
  void pushNativeWindowTitle(visibleWorkspaceName.value);
});

effect(() => {
  syncWorkspaceAppBadge();
});

effect(() => {
  if (drawerGrouping.value === 'ongoing' && untracked(() => threadFilterPanelOpen.value)) closeThreadFilterPanel();
});

effect(() => {
  const ids = appIdsMissingFromFacets.value;
  if (ids.length > 0) untracked(() => reloadFacetsForApps(ids));
});

effect(() => {
  const appId = viewingNotification.value?.app_id;
  if (appId && untracked(() => appById(appId).status) === 'failed') forgetAppById(appId);
});

effect(() => {
  const appId = viewingNotification.value?.app_id;
  const apps = appsList.value;
  if (!appId || apps.status !== 'loaded' || apps.data.some(a => a.id === appId)) return;
  if (appById(appId).status === 'not-loaded') untracked(() => void readAppsById([appId]));
});

// The retired status filter's choice, and the open group of the grouping once
// called Status. A device that held either lands on Folders. A temporary
// measure: docs/temporary-measures.md § Retired drawer keys cleared at load.
localStorage.removeItem('lucidos-alt-view');
localStorage.removeItem('lucidos-drawer-open-status-group');

localStorage.removeItem('lucidos-model');
localStorage.removeItem('lucidos-reasoning-effort');
localStorage.removeItem('lucidos-unread-count');
localStorage.removeItem('lucidos-input-target');

effect(() => {
  localStorage.setItem('lucidos-input-mode', JSON.stringify(inputMode.value));
});

effect(() => {
  localStorage.setItem(ANIMATION_SPEED_STORAGE_KEY, String(animationSpeed.value));
});

installMotionAttribute();

installThemeEffectsAttribute();

effect(() => {
  document.documentElement.style.setProperty('--duration-scale', String(durationScale.value));
});

effect(() => {
  persistTurnControl(STEPS_EXPANDED_KEY, stepsExpanded.value);
});

effect(() => {
  persistTurnControl(DETAILS_EXPANDED_KEY, detailsExpanded.value);
});

effect(() => {
  localStorage.setItem('lucidos-expanded-folders', JSON.stringify([...expandedFolders.value]));
});

effect(() => {
  localStorage.setItem('lucidos-thread-drawer-open', String(threadDrawerOpen.value));
});

effect(() => {
  localStorage.setItem('lucidos-coding-agent-last-scope', JSON.stringify(selectedScope.value));
});

effect(() => {
  localStorage.setItem(NOTIFICATIONS_FILTER_STORAGE_KEY, notificationsFilter.value);
});

effect(() => {
  localStorage.setItem('lucidos-collapsed-exchanges', JSON.stringify([...collapsedExchanges.value]));
});

effect(() => {
  localStorage.setItem('lucidos-collapsed-initiators', JSON.stringify([...collapsedInitiators.value]));
});

effect(() => {
  localStorage.setItem(OPEN_SUB_AGENT_GROUPS_KEY, JSON.stringify([...openSubAgentGroups.value]));
});

effect(() => {
  localStorage.setItem('lucidos-file-preview-source', String(filePreviewSource.value));
});

effect(() => {
  localStorage.setItem('lucidos-diff-side-by-side', String(diffSideBySide.value));
});

effect(() => {
  localStorage.setItem('lucidos-file-preview-wrap', String(filePreviewWrap.value));
});

let lastPreviewFile: string | null = previewFile.value;
effect(() => {
  const path = previewFile.value;
  if (path !== lastPreviewFile) {
    lastPreviewFile = path;
    filePreviewEditing.value = false;
    diffWholeFile.value = null;
  }
});

let lastViewingNotificationId: string | null = viewingNotification.value?.id ?? null;
effect(() => {
  const id = viewingNotification.value?.id ?? null;
  if (id === lastViewingNotificationId) return;
  const closed = id === null && lastViewingNotificationId !== null;
  lastViewingNotificationId = id;
  if (closed) untracked(() => onNotificationDetailClosed());
});

effect(() => {
  const id = repoSelectedChangeId.value;
  if (id) {
    writeWindowItem(SELECTED_CHANGE_KEY, id);
  } else {
    removeWindowItem(SELECTED_CHANGE_KEY);
  }
});

effect(() => {
  if (!clientRefreshing.value) return;
  untracked(() => {
    removeToast('update-available');
    showToast('Refreshing...', 'info', { key: 'refreshing', spinning: true, dismissable: false, showWhileUnavailable: true });
  });
});

const RESTART_TIMEOUT_MS = 300_000;
let restartTimer: ReturnType<typeof setTimeout> | null = null;
effect(() => {
  if (engineRestarting.value) {
    if (restartTimer === null) {
      restartTimer = setTimeout(() => {
        restartTimer = null;
        void handleRestartTimeout();
      }, RESTART_TIMEOUT_MS);
    }
  } else if (restartTimer !== null) {
    clearTimeout(restartTimer);
    restartTimer = null;
  }
});

installAppAppearanceSync();
installAppFullscreenWindowSync();

installNotificationToastLifetime();

installUnregisteredRepoTargetReset();

installLiveUtteranceRow();
