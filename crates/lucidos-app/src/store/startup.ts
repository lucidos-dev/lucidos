import { subscribeEngineVersionStatus } from './engineVersionStatus';
import { dismissedEngineVersion } from '../hooks/sw-update';
import { noteCrashWorker } from '../utils/crashWorker';
import { provideReleaseCheck, refreshReleaseCheckIfShown } from './releaseCheckHandle';
import { startAppUpdateProgress, stopAppUpdateProgress } from './actions/appUpdateProgressEvents';
import { startDeferredBackgroundChecks } from './deferredBackgroundChecks';
import { installDeferredInteractiveDelivery } from './interactiveDeliveryStartup';
import { loadSpendCapHolds } from './actions/spendCapHolds';
import { pruneClosedWindows, readWindowItem } from '../utils/windowStorage';
import { installRestoreRuntime } from './restoreRuntime';
import { checkConnection, handleResume, bounceToPickerIfStranded } from './actions/connection';
import { loadArtifacts } from './actions/artifacts';
import {
  loadUnreadNotifications,
  loadNotifications,
  refreshActiveNotificationsTab,
} from './actions/notifications';
import { syncWorkspaceAppBadge, refreshOtherWorkspacesUnread } from './actions/app-badge';
import { loadApps } from './actions/apps';
import { loadInstalledPlugins } from './actions/plugins';
import { loadPluginCatalog } from './actions/plugin-marketplaces';
import { loadCredentials } from './actions/credentials';
import { loadLastSuccessfulBackup } from './actions/backup';
import { loadChatLadder, loadChatModels } from './actions/models';
import { loadDevices, registerCurrentDevice } from './actions/devices';
import { loadPairedDevices } from './actions/pairedDevices';
import { loadGatewayUsers } from './actions/gatewayUsers';
import { loadTriggers, loadHistoricalTriggers } from './actions/triggers';
import { loadTriggerGroups } from './actions/triggerGroups';
import { loadThreadQueue } from './actions/threadQueue';
import { loadRepositories } from './actions/repositoriesLoader';
import { reopenDashboardPanelFromHistory } from './actions/thread-navigation';
import { loadPreferences, preferencesSettled, flushPendingPreferenceWrites } from './actions/preferences';
import { loadPinnedApps } from './actions/pinnedApps';
import { loadReleaseNotices } from './actions/releaseNotices';
import { loadReaderDevices } from './actions/reader-devices';
import { loadUsers } from './actions/users';
import { loadWebhookIngress } from './actions/webhookIngress';
import { loadWebhookRefusals } from './actions/webhookRefusals';
import { loadWorkspaceDisplayName } from './actions/workspace-label';
import { connectThreadEvents, disconnectThreadEvents } from './actions/thread-stream';
import { loadAllThreads, loadFilterFacets } from './actions/thread-loading';
import { refreshPushSubscription, recoverServiceWorker, unregisterServiceWorker } from './actions/push';
import { createSwLivenessProbe } from '../utils/swLivenessProbe';
import { createSwFetchPathProbe } from '../utils/swFetchPathProbe';
import { setupNativePushTapRouting } from './actions/native-push';
import { startDevicePresenceTracking } from './actions/device-presence';
import {
  loadEmbeddingModelStatus,
  subscribeToTailscaleServeProgress,
  unsubscribeFromTailscaleServeProgress,
} from './actions/backgroundActivity';
import { isTauri } from '../utils/platform';
import { startPageHeartbeat } from '../utils/heartbeat';
import { refreshChangesState, restoreRestartState } from './actions/chat-changes';
import { offerWithheldThread, openHomeOnColdStart, releaseWithheldToLiveWindow } from './actions/coldStartHome';
import { announceWindow, installWindowPeers } from './windowPeers';
import { oauthAuthFlow } from './actions/oauth';
import { installRestoreGuard, takeWithheldThread } from './restoreGuard';
import { installDevWindowFollow } from './actions/dev-window';
import { restoreRepoSelectionFromStorage } from './actions/repoSelectionRestore';
import { installMarkdownImageRetry } from '../utils/markdownImageRetry';
import { installMarkdownImageFallback } from '../utils/markdownImageFallback';
import { effect, untracked } from '@preact/signals';
import { markBoot } from '../utils/bootTiming';
import { activeMenuItem, backupStatusVersion, connectionStatus, intelligenceLadderVersion, notificationsFilter, settingsSubview, serviceWorkerBuildId, threadsLoaded, showToast, CONNECTION_POLL_INTERVAL_MS, FOCUSED_THREAD_KEY, focusedThreadId, setFocusedThread, currentApp } from './store';
import { requestServiceWorkerBuildId } from '../hooks/sw-update';
import { syncClientUpdateFromBuild } from './actions/client-update';
import {
  parseDeepLinkFromSwMessage,
  hasDeepLinkParams,
} from './actions/notification-deeplink';
import { dispatchDeepLink } from './actions/in-app-notification-toast';
import { setupHashDeeplinkRouting } from './actions/hash-deeplink-router';
import { reportStartupKind, startLivenessTracking } from '../utils/liveness';
import { postClientLog } from '../utils/clientLog';
import { createLeadingEdgeGate } from '../utils/leadingEdgeGate';
import { flushUndeliveredComposeDrafts } from './actions/compose';
import {
  startFrameCapabilityRenewal,
  stopFrameCapabilityRenewal,
} from './actions/frame-capability';
import { withBase, SCOPE_PATH } from '../utils/basePath';
import { workspaceServiceWorkerUrl } from '../utils/workspaceServiceWorker';
import { isDevServerBundle, DEV_SERVER_SW_REASON } from '../utils/devServerBundle';

const COLD_START_BOUNCE_MS = 10_000;
const SW_PROBE_INTERVAL_MS = 5 * 60 * 1000;
const CROSS_WORKSPACE_BADGE_INTERVAL_MS = 60 * 1000;
const RESUME_COALESCE_MS = 1000;

function visibleOnlyInterval(tick: () => void, everyMs: number): { start: () => void; stop: () => void } {
  let id: number | null = null;
  return {
    start() {
      if (id !== null) return;
      id = window.setInterval(tick, everyMs);
    },
    stop() {
      if (id === null) return;
      clearInterval(id);
      id = null;
    },
  };
}

export function startClient(): () => void {
  installRestoreRuntime();
  startAppUpdateProgress();
  const backgroundChecks = startDeferredBackgroundChecks();
  const stopEngineVersionReads = subscribeEngineVersionStatus(
    () => backgroundChecks.refreshEngine(), () => preferencesSettled.value, dismissedEngineVersion);
  provideReleaseCheck(async () => { backgroundChecks.refreshRelease(); });
  reportStartupKind();
  const stopLiveness = startLivenessTracking();
  markBoot('clientStarted');
  const stopBootMarks = effect(() => {
    if (connectionStatus.value === 'connected') markBoot('connected');
    if (threadsLoaded.value) markBoot('threadsLoaded');
  });

  startFrameCapabilityRenewal();

  pruneClosedWindows();
  const savedThreadId = readWindowItem(FOCUSED_THREAD_KEY);
  if (savedThreadId) {
    setFocusedThread(savedThreadId);
  }
  reopenDashboardPanelFromHistory();
  const stopColdStartHome = openHomeOnColdStart(savedThreadId !== null);
  const authorizingNow = () => {
    const flow = oauthAuthFlow.peek();
    return flow === null ? null : { provider: flow.provider };
  };
  const stopWindowPeers = installWindowPeers(() => ({
    focusedThreadId: focusedThreadId.peek(),
    appId: currentApp.peek()?.id ?? null,
    authorizing: authorizingNow(),
  }));
  const stopInteractiveDelivery = installDeferredInteractiveDelivery();
  const stopAnnouncingWindow = effect(() => {
    void focusedThreadId.value;
    void currentApp.value;
    void oauthAuthFlow.value;
    announceWindow();
  });
  const withheld = takeWithheldThread();
  const stopReleasingWithheld = releaseWithheldToLiveWindow(withheld);
  const stopRestoreOffer = offerWithheldThread(withheld?.threadId ?? null);
  const stopRestoreGuard = installRestoreGuard(() => focusedThreadId.peek());

  restoreRestartState();

  checkConnection().then((connected) => {
    if (connected) {
      connectThreadEvents();
    }
  }).catch(() => {  });
  void loadUnreadNotifications();
  void loadReaderDevices();
  void loadUsers();
  void refreshOtherWorkspacesUnread();
  const filterBeforePreferences = notificationsFilter.value;
  if (activeMenuItem.value === 'notifications' && filterBeforePreferences === 'all') {
    void loadNotifications();
  }
  loadPreferences().then(() => {
    if (
      activeMenuItem.value === 'notifications' &&
      notificationsFilter.value !== filterBeforePreferences
    ) {
      void refreshActiveNotificationsTab();
    }
    void syncClientUpdateFromBuild();
  }).catch(() => {  });
  void loadPinnedApps();
  void loadChatModels();
  const stopChatLadderReads = effect(() => {
    void intelligenceLadderVersion.value;
    untracked(() => { void loadChatLadder(); });
  });
  const stopLastBackupReads = effect(() => {
    void backupStatusVersion.value;
    untracked(() => { void loadLastSuccessfulBackup(); });
  });
  void loadReleaseNotices();
  void loadWorkspaceDisplayName();
  void loadEmbeddingModelStatus();
  void subscribeToTailscaleServeProgress();
  void loadWebhookIngress();
  void loadWebhookRefusals();
  loadAllThreads().catch(() => {
    setTimeout(() => loadAllThreads().catch(() => {}), 3000);
  });
  registerCurrentDevice();
  const stopDevicePresence = startDevicePresenceTracking();
  let stopNativeTap: (() => void) | null = null;
  let nativeTapCanceled = false;
  setupNativePushTapRouting()
    .then((un) => {
      if (nativeTapCanceled) un();
      else stopNativeTap = un;
    })
    .catch(() => {  });
  refreshChangesState();

  const stopMarkdownImageRetry = installMarkdownImageRetry();
  const stopMarkdownImageFallback = installMarkdownImageFallback();

  const stopHashRouting = setupHashDeeplinkRouting();

  loadArtifacts();
  loadApps();
  void loadPluginCatalog();
  void loadInstalledPlugins();
  loadTriggers();
  loadHistoricalTriggers();
  loadThreadQueue();
  loadTriggerGroups();
  loadRepositories();
  void loadFilterFacets();
  void loadSpendCapHolds();
  loadDevices();
  void loadPairedDevices();
  void loadGatewayUsers();

  const tab = activeMenuItem.value;
  if (tab === 'settings') {
    if (settingsSubview.value === 'accounts') loadCredentials();
  }
  if (tab === 'files') {
    restoreRepoSelectionFromStorage();
  }
  let updateFoundReg: ServiceWorkerRegistration | null = null;
  let updateFoundHandler: ((this: ServiceWorkerRegistration) => void) | null = null;
  if (!window.isSecureContext && !isTauri()) {
    console.warn(
      `[Startup] Insecure origin (${location.origin}): service worker + push notifications unavailable. Open Lucidos over https:// or localhost (SSH tunnel / tailscale serve).`,
    );
  }
  if (isDevServerBundle()) {
    console.warn(`[Startup] ${DEV_SERVER_SW_REASON}`);
  } else if ('serviceWorker' in navigator) {
    navigator.serviceWorker.register(workspaceServiceWorkerUrl(), { scope: SCOPE_PATH, updateViaCache: 'none' })
      .then(async () => {
        await refreshPushSubscription(await navigator.serviceWorker.ready);
      })
      .catch(() => showToast('Service worker registration failed. Push notifications may not work.', 'error'));

    function onUpdateFound(this: ServiceWorkerRegistration) {
      noteCrashWorker(5);
      const newWorker = this.installing;
      if (!newWorker) return;
      newWorker.addEventListener('statechange', () => {
        if (newWorker.state === 'activated') void syncClientUpdateFromBuild();
      });
    }
    updateFoundHandler = onUpdateFound;
    navigator.serviceWorker.ready.then(reg => {
      if (stopped) return;
      updateFoundReg = reg;
      reg.addEventListener('updatefound', onUpdateFound);
    }).catch(() => {  });

    navigator.serviceWorker.addEventListener('message', onServiceWorkerMessage);

    requestServiceWorkerBuildId();
    navigator.serviceWorker.addEventListener('controllerchange', onControllerChange);

  }

  if (!isDevServerBundle()) void syncClientUpdateFromBuild();

  let stopped = false;
  const swLiveness = createSwLivenessProbe({
    ping: () => {
      const controller = navigator.serviceWorker?.controller;
      if (!controller) return false;
      controller.postMessage({ type: 'lucidos:ping' });
      return true;
    },
    onState: noteCrashWorker,
    recover: recoverServiceWorker,
    onRecovered: () => showToast('Notifications repaired: the service worker was unresponsive', 'info'),
  });
  const swFetchPath = createSwFetchPathProbe({
    hasController: () => !!navigator.serviceWorker?.controller,
    fetchUrl: (url, signal) => fetch(url, { signal, cache: 'no-store' }),
    healthUrl: withBase('/api/v1/health'),
    recover: async () => {
      await unregisterServiceWorker();
      window.location.reload();
    },
    onSuspected: () => postClientLog('sw', 'fetch_path_slow', {}),
    onWedged: () => postClientLog('sw', 'fetch_path_wedged', {}),
  });
  let swHealthRun: Promise<void> | null = null;
  const runSwHealth = (steps: () => Promise<void>) => (swHealthRun ??= (async () => {
    try {
      await steps();
    } finally {
      swHealthRun = null;
    }
  })());
  const checkSwHealth = () => runSwHealth(async () => {
    await swLiveness.check();
    await swFetchPath.check();
  });
  const bootFetchPathCheck = runSwHealth(() => swFetchPath.check())
    .catch(() => {  });

  function onControllerChange() {
    noteCrashWorker(3);
    requestServiceWorkerBuildId();
  }

  function onServiceWorkerMessage(event: MessageEvent) {
    const data = event.data as { type?: unknown; target?: unknown; buildId?: unknown; instance?: unknown } | null;
    if (!data || typeof data !== 'object') return;
    if (data.type === 'lucidos:deep-link') {
      const target = parseDeepLinkFromSwMessage(data.target);
      if (!target || !hasDeepLinkParams(target)) return;
      dispatchDeepLink(target);
      return;
    }
    if (data.type === 'lucidos:pong') {
      swLiveness.notePong(data.instance);
      return;
    }
    if (data.type === 'lucidos:build-id') {
      if (typeof data.buildId === 'string') {
        serviceWorkerBuildId.value = data.buildId;
        void syncClientUpdateFromBuild();
      }
      return;
    }
  }

  const initialHealthCheck = window.setTimeout(() => {
    bootFetchPathCheck
      .then(checkSwHealth)
      .catch(() => {  });
  }, 5000);

  function onResume() {
    if (!threadsLoaded.value) return;
    handleResume();
    syncWorkspaceAppBadge();
    void loadUnreadNotifications();
    void refreshOtherWorkspacesUnread();
    void flushPendingPreferenceWrites();
    void syncClientUpdateFromBuild().then(async stale => {
      if (!stale) return;
      const reg = await navigator.serviceWorker?.getRegistration();
      if (reg) {
        noteCrashWorker(4);
        await reg.update();
      }
    }).catch(() => {});
    refreshReleaseCheckIfShown();
    void loadEmbeddingModelStatus();
    void loadWebhookIngress();
    void loadWebhookRefusals();
    backgroundChecks.refreshSlowness();
    void loadWorkspaceDisplayName();
    flushUndeliveredComposeDrafts();
    checkSwHealth().catch(() => {  });
  }

  const resumeGate = createLeadingEdgeGate(RESUME_COALESCE_MS);
  function onResumeCoalesced() {
    if (!resumeGate.allow()) return;
    onResume();
  }

  const swProbe = visibleOnlyInterval(() => {
    checkSwHealth().catch(() => {  });
  }, SW_PROBE_INTERVAL_MS);

  const crossWorkspaceBadge = visibleOnlyInterval(() => {
    void refreshOtherWorkspacesUnread();
  }, CROSS_WORKSPACE_BADGE_INTERVAL_MS);

  function handleVisibilityChange() {
    if (document.visibilityState === 'visible') {
      onResumeCoalesced();
      swProbe.start();
      crossWorkspaceBadge.start();
    } else {
      swProbe.stop();
      crossWorkspaceBadge.stop();
    }
  }
  document.addEventListener('visibilitychange', handleVisibilityChange);
  window.addEventListener('focus', onResumeCoalesced);
  window.addEventListener('pageshow', onResumeCoalesced);
  if (document.visibilityState === 'visible') {
    swProbe.start();
    crossWorkspaceBadge.start();
  }

  const connectionInterval = setInterval(() => {
    checkConnection();
  }, CONNECTION_POLL_INTERVAL_MS);

  const coldStartBounceTimer = window.setTimeout(() => {
    bounceToPickerIfStranded();
  }, COLD_START_BOUNCE_MS);

  const stopHeartbeat = isTauri() ? startPageHeartbeat() : null;

  const stopDevWindowFollow = installDevWindowFollow();

  return () => {
    stopped = true;
    swLiveness.stop();
    swFetchPath.stop();
    stopLiveness();
    stopBootMarks();
    stopChatLadderReads();
    stopLastBackupReads();
    stopEngineVersionReads();
    stopColdStartHome();
    stopRestoreOffer();
    stopReleasingWithheld();
    stopAnnouncingWindow();
    stopInteractiveDelivery();
    stopWindowPeers();
    stopRestoreGuard();
    stopDevWindowFollow();
    stopFrameCapabilityRenewal();
    clearInterval(connectionInterval);
    clearTimeout(coldStartBounceTimer);
    swProbe.stop();
    crossWorkspaceBadge.stop();
    stopHeartbeat?.();
    backgroundChecks.stop();
    stopAppUpdateProgress();
    unsubscribeFromTailscaleServeProgress();
    clearTimeout(initialHealthCheck);
    stopHashRouting();
    navigator.serviceWorker?.removeEventListener('message', onServiceWorkerMessage);
    navigator.serviceWorker?.removeEventListener('controllerchange', onControllerChange);
    if (updateFoundReg && updateFoundHandler) {
      updateFoundReg.removeEventListener('updatefound', updateFoundHandler);
    }
    disconnectThreadEvents();
    stopDevicePresence();
    nativeTapCanceled = true;
    stopNativeTap?.();
    stopMarkdownImageRetry();
    stopMarkdownImageFallback();
    document.removeEventListener('visibilitychange', handleVisibilityChange);
    window.removeEventListener('focus', onResumeCoalesced);
    window.removeEventListener('pageshow', onResumeCoalesced);
  };
}
