import { useEffect } from 'preact/hooks';
import { checkConnection, handleResume } from '../store/actions/connection';
import { loadArtifacts, openUrl } from '../store/actions/artifacts';
import { refreshUnreadCount, loadNotifications } from '../store/actions/notifications';
import { loadApps } from '../store/actions/apps';
import { loadCredentials } from '../store/actions/credentials';
import { loadDevices, registerCurrentDevice } from '../store/actions/devices';
import { loadTriggers, loadHistoricalTriggers } from '../store/actions/triggers';
import { loadTriggerGroups } from '../store/actions/triggerGroups';
import { loadRepositories } from '../store/actions/chat';
import { loadPreferences } from '../store/actions/preferences';
import { loadPinnedApps } from '../store/actions/pinnedApps';
import { connectThreadEvents, disconnectThreadEvents } from '../store/actions/thread-sync';
import { loadAllThreads, loadFilterFacets } from '../store/actions/thread-loading';
import { refreshPushSubscription, recoverServiceWorker } from '../store/actions/push';
import { startDevicePresenceTracking } from '../store/actions/device-presence';
import { startScrollVisibilityHandler } from '../components/chat/scrollState';
import { isTauri } from '../utils/platform';
import { invoke } from '../utils/tauri';
import { refreshChangesState, restoreRestartToast } from '../store/actions/chat-changes';
import { restoreRepoSelectionFromStorage } from '../store/actions/repositories';
import { openThreadAcrossWorkspaces } from '../store/actions/cross-workspace';
import { CHECK_ICON, COPY_ICON } from '../utils/markedConfig';
import { activeMenuItem, settingsSubview, updateAvailable, serviceWorkerBuildId, threadsLoaded, showToast, showConfirm, FOCUSED_THREAD_KEY, setFocusedThread } from '../store/store';
import { shouldShowSwUpdateToast, markSwUpdateDismissed, requestServiceWorkerBuildId } from './sw-update';
import {
  parseDeepLinkFromSwMessage,
  hasDeepLinkParams,
} from '../store/actions/notification-deeplink';
import { dispatchDeepLink } from '../store/actions/in-app-notification-toast';
import { setupHashDeeplinkRouting } from '../store/actions/hash-deeplink-router';
import { reportStartupKind, startLivenessTracking } from '../utils/liveness';

const CONNECTION_POLL_INTERVAL = 5000;
// Catches Chrome's idle-SW LRU eviction (Chromium issue #370536109:
// notificationclick silently dropped) without churning the recovery path,
// which is itself cooldown-debounced.
const SW_PROBE_INTERVAL_MS = 5 * 60 * 1000;

export function useStartup(): void {
  useEffect(() => {
    // Diagnostic-only: classify this load (cold / bg_resume / reload_clean /
    // nav_clean / likely_crash) against the prior heartbeat. POSTs one log
    // line then starts the heartbeat ticker. See utils/liveness.ts.
    reportStartupKind();
    const stopLiveness = startLivenessTracking();

    // Restore focused thread from localStorage (set at signal init, reinforce here).
    // setFocusedThread short-circuits when the value is unchanged, so this is a
    // no-op on cold start where the signal initializer already populated it.
    const savedThreadId = localStorage.getItem(FOCUSED_THREAD_KEY);
    if (savedThreadId) {
      setFocusedThread(savedThreadId);
    }

    // Initial loads
    checkConnection().then((connected) => {
      if (connected) {
        connectThreadEvents();
      }
    }).catch(() => { /* checkConnection swallows internally; satisfy fail-fast rule */ });
    refreshUnreadCount();
    loadPreferences().then(() => {
      // Notifications must load after preferences so the persisted filter is applied
      if (activeMenuItem.value === 'notifications') loadNotifications();
    }).catch(() => { /* loadPreferences sets Loadable failed internally — UI shows the error */ });
    void loadPinnedApps();
    loadAllThreads().catch(() => {
      // Retry after 3s — covers transient network failures on initial load.
      // If this also fails, the 5s health poll will keep retrying.
      setTimeout(() => loadAllThreads().catch(() => {}), 3000);
    });
    registerCurrentDevice();
    // Thread focus is now reported live in the PresenceCheck pong, so the
    // periodic POST loop is gone (see system-knowhow/notifications.md §3).
    // Device-presence still pings so the engine knows which devices count
    // as candidates for the PresenceCheck broadcast.
    const stopDevicePresence = startDevicePresenceTracking();
    // Capture wasAtBottom on tab-hide so we can re-pin to the new bottom on
    // return — covers the streaming-while-tabbed-away → frozen-scroll bug.
    const stopScrollVisibility = startScrollVisibilityHandler();
    restoreRestartToast();  // Immediate — show from localStorage before async API
    refreshChangesState();

    // Global click handler for thread links, external URLs, and copy buttons
    function onGlobalClick(e: MouseEvent) {
      const target = e.target as HTMLElement;

      const threadLink = target.closest('.thread-link') as HTMLElement | null;
      if (threadLink) {
        e.preventDefault();
        const threadId = threadLink.getAttribute('data-thread-id');
        if (!threadId) return;
        // Cross-workspace links carry data-thread-workspace; the shared router
        // hops to that workspace's UI (its thread isn't in our threadMap) and
        // focuses in place otherwise.
        const linkWorkspace = threadLink.getAttribute('data-thread-workspace');
        openThreadAcrossWorkspaces(linkWorkspace ?? undefined, threadId);
        return;
      }

      // Copy buttons (copyable blocks and fenced code blocks)
      const copyBtn = target.closest('.copy-btn') as HTMLElement | null;
      if (copyBtn) {
        e.preventDefault();
        e.stopPropagation();
        // Resolve text: copyable-block uses data attribute, code-block uses textContent
        let text: string | null = null;
        const copyableBlock = copyBtn.closest('.copyable-block') as HTMLElement | null;
        if (copyableBlock) {
          text = copyableBlock.getAttribute('data-copy-text');
        } else {
          const wrapper = copyBtn.closest('.code-block-wrapper') as HTMLElement | null;
          text = wrapper?.querySelector('pre code')?.textContent ?? null;
        }
        if (text == null) return;
        navigator.clipboard.writeText(text).then(() => {
          copyBtn.innerHTML = CHECK_ICON;
          copyBtn.classList.add('copy-btn-copied');
          setTimeout(() => {
            copyBtn.innerHTML = COPY_ICON;
            copyBtn.classList.remove('copy-btn-copied');
          }, 1500);
        }).catch(() => {
          showToast('Failed to copy to clipboard', 'error');
        });
        return;
      }

      // External links not handled by component-level handlers
      const anchor = target.closest('a[href]') as HTMLAnchorElement | null;
      if (anchor) {
        const href = anchor.getAttribute('href');
        if (href && /^https?:\/\//.test(href)) {
          e.preventDefault();
          openUrl(href);  // Panel in Tauri, new tab in browser
        }
      }
    }
    document.addEventListener('click', onGlobalClick);

    // Cold-start, warm hashchange, AND resume (visibilitychange / focus /
    // pageshow) all dispatch through one shared router so iOS PWA — which
    // misses the hashchange that pairs with Safari's declarative-push URL
    // update while the JS is suspended — still routes the deep link on wake.
    // See store/actions/hash-deeplink-router.ts for the full rationale.
    const stopHashRouting = setupHashDeeplinkRouting();

    loadArtifacts();
    loadApps();
    // Triggers are global (thread filter dropdown + form titles), not tab-scoped.
    // Without this eager load, the dropdown lies on cold start to a non-triggers
    // tab — every trigger in the registry shows as "(deleted)" because the
    // registry is empty. Same shape as loadApps above.
    loadTriggers();
    loadHistoricalTriggers();
    // Trigger groups are tiny and pre-loading them avoids the panel painting
    // every trigger under "Ungrouped" while it waits for the group list to
    // arrive. Same eager-load reasoning as triggers.
    loadTriggerGroups();
    // Repositories power the Claude Code parent's expandable child rows in the
    // thread filter dropdown. Same eager-load reasoning as triggers — without
    // it the dropdown lies on cold start to a non-Settings tab.
    loadRepositories();
    // Filter facets = the complete set of triggers/repos/apps that have a
    // thread, so the "Show" dropdown lists them all (not just facets in the
    // loaded window). loadAllThreads also refreshes these, but call it eagerly
    // in case the thread fetch is slow or fails.
    void loadFilterFacets();

    // Load data for the restored active menu item (switchMenuItem isn't called on reload)
    const tab = activeMenuItem.value;
    if (tab === 'settings') {
      loadDevices();
      if (settingsSubview.value === 'accounts') loadCredentials();
    }
    if (tab === 'files') {
      restoreRepoSelectionFromStorage();
    }
    // Register/update the service worker on every load so the browser
    // picks up new sw.js versions (skipWaiting activates them immediately).
    // Also re-subscribe push to keep the endpoint fresh.
    if ('serviceWorker' in navigator) {
      // Capture BEFORE register() — clients.claim() in sw.js sets the
      // controller, so after register() this would always be true.
      const hadController = !!navigator.serviceWorker.controller;

      navigator.serviceWorker.register('/sw.js', { updateViaCache: 'none' })
        .then((reg) => {
          refreshPushSubscription(reg);
        })
        .catch(() => showToast('Service worker registration failed — push notifications may not work', 'error'));

      // Show update toast when the browser finds a genuinely new service worker.
      // `updatefound` fires on BOTH first install and genuine updates. We only
      // want the toast for genuine updates (when there was already a controller).
      // A sessionStorage flag prevents re-showing after the user dismisses or
      // clicks Refresh (consumed on next check so future updates still show).
      function onUpdateFound(this: ServiceWorkerRegistration) {
        const newWorker = this.installing;
        if (!newWorker) return;
        newWorker.addEventListener('statechange', () => {
          if (newWorker.state === 'activated' && shouldShowSwUpdateToast(hadController)) {
            updateAvailable.value = true;
            showToast('New version available', 'info', {
              key: 'update-available',
              action: {
                label: 'Refresh',
                onClick: () => {
                  markSwUpdateDismissed();
                  window.location.reload();
                },
              },
            });
          }
        });
      }
      navigator.serviceWorker.ready.then(reg => {
        reg.addEventListener('updatefound', onUpdateFound);
      }).catch(() => { /* SW not ready in this environment — update toast is best-effort */ });

      // Listens for two SW-originated messages: the deep-link delivery (how a
      // notification tap routes an already-open tab — routeToDeepLink in sw.js
      // posts the structured deep link rather than fragment-navigating; see
      // system-knowhow/notifications.md §4.5) and the liveness pong.
      navigator.serviceWorker.addEventListener('message', onServiceWorkerMessage);

      // Surface the active SW's BUILD_ID in the control panel (debugging aid for
      // "did the new build's SW take over?"). Query now, and again whenever a new
      // SW claims the page so the shown id tracks the live worker.
      requestServiceWorkerBuildId();
      navigator.serviceWorker.addEventListener('controllerchange', requestServiceWorkerBuildId);
    }

    // Closure-local so a hot-reload remount starts fresh.
    let lastPongAt = 0;
    let lastRecoveryAt = 0;
    let probeInFlight = false;
    let unmounted = false;
    // Resolver for the in-flight probe's pong wait — the SW message handler
    // calls this to short-circuit the 5s timeout when a pong actually arrives.
    let pongResolver: (() => void) | null = null;
    // Cap recovery rate so a misbehaving SW can't churn push subscription
    // endpoints in the backend.
    const RECOVERY_COOLDOWN_MS = 60_000;
    const PROBE_TIMEOUT_MS = 5000;

    function onServiceWorkerMessage(event: MessageEvent) {
      const data = event.data as { type?: unknown; target?: unknown; buildId?: unknown } | null;
      if (!data || typeof data !== 'object') return;
      if (data.type === 'lucidos:deep-link') {
        const target = parseDeepLinkFromSwMessage(data.target);
        if (!target || !hasDeepLinkParams(target)) return;
        dispatchDeepLink(target);
        return;
      }
      if (data.type === 'lucidos:pong') {
        lastPongAt = Date.now();
        pongResolver?.();
        return;
      }
      if (data.type === 'lucidos:build-id') {
        if (typeof data.buildId === 'string') serviceWorkerBuildId.value = data.buildId;
        return;
      }
    }

    async function checkSwHealth() {
      if (probeInFlight || unmounted) return;
      const controller = navigator.serviceWorker?.controller;
      if (!controller) return;
      if (Date.now() - lastRecoveryAt < RECOVERY_COOLDOWN_MS) return;

      probeInFlight = true;
      try {
        const sentAt = Date.now();
        controller.postMessage({ type: 'lucidos:ping' });
        // Race the timeout against the pong resolver so a healthy SW (pong
        // in milliseconds) doesn't hold the probe open for the full timeout.
        // Clear the timer on early resolve — left dangling, it would queue
        // one wasted task per probe (~12/hour with the visible-tab probe).
        let timeoutHandle: ReturnType<typeof setTimeout> | null = null;
        await new Promise<void>((resolve) => {
          pongResolver = () => { if (timeoutHandle !== null) clearTimeout(timeoutHandle); resolve(); };
          timeoutHandle = setTimeout(() => { timeoutHandle = null; resolve(); }, PROBE_TIMEOUT_MS);
        });
        pongResolver = null;
        if (unmounted) return;
        if (lastPongAt >= sentAt) return;
        lastRecoveryAt = Date.now();
        // Let recoverServiceWorker throws bubble to the call-site `.catch`.
        // The user-facing repair toast only fires on the success branch — a
        // failed recovery genuinely repaired nothing, and any persistent
        // SW wedge resurfaces on the next probe (5-minute cadence) or next
        // page reload. RECOVERY_COOLDOWN_MS gates retries, so no spam loop.
        await recoverServiceWorker();
        if (!unmounted) {
          showToast('Notifications repaired — service worker was unresponsive', 'info');
        }
      } finally {
        probeInFlight = false;
      }
    }

    // Cold-start delay: lets the initial register + activate finish so the
    // probe doesn't time out against a pre-activation SW that wasn't broken.
    const initialHealthCheck = window.setTimeout(() => {
      checkSwHealth().catch(() => { /* best-effort recovery; next probe retries */ });
    }, 5000);

    function onAppFrameMessage(event: MessageEvent) {
      const data = event.data as {
        type?: unknown;
        id?: unknown;
        payload?: { title?: unknown; message?: unknown; okLabel?: unknown; cancelLabel?: unknown; danger?: unknown };
      } | null;
      if (!data || typeof data !== 'object') return;
      if (data.type !== 'lucidos:ui:confirm') return;
      if (typeof data.id !== 'string') return;
      const payload = data.payload;
      if (!payload || typeof payload !== 'object') return;
      if (typeof payload.message !== 'string' || payload.message.length === 0) return;

      // Reject messages from any iframe that isn't a current app iframe,
      // so nested iframes (embeds, ads) can't trigger host modals.
      const source = event.source as Window | null;
      if (!source) return;
      const frames = document.querySelectorAll<HTMLIFrameElement>('iframe[data-role="app-ui-frame"]');
      const known = Array.from(frames).some((f) => f.contentWindow === source);
      if (!known) return;

      const title = typeof payload.title === 'string' ? payload.title : undefined;
      const okLabel = typeof payload.okLabel === 'string' && payload.okLabel.length > 0 ? payload.okLabel : 'Confirm';
      const cancelLabel = typeof payload.cancelLabel === 'string' && payload.cancelLabel.length > 0 ? payload.cancelLabel : 'Cancel';
      const variant: 'danger' | 'default' = payload.danger === true ? 'danger' : 'default';

      showConfirm(payload.message, okLabel, { title, cancelLabel, variant }).then((ok) => {
        try {
          source.postMessage({ type: 'lucidos:ui:confirm:result', id: data.id, ok }, '*');
        } catch {
          // Source iframe may have unloaded — drop the reply silently.
        }
      }).catch(() => { /* showConfirm rejection — drop, modal already closed */ });
    }
    window.addEventListener('message', onAppFrameMessage);

    // On iOS PWA, the page doesn't reload when returning from background.
    // Reconnect SSE and check for SW updates. Notification deep-links arrive
    // via SW client.navigate() — hashchange (warm) or URL params on cold load.
    function onResume() {
      if (!threadsLoaded.value) return;
      // handleResume health-checks the engine first. If unreachable, it defers
      // the sync to the 5s health poll (which calls checkConnection, which
      // picks up the deferred resume once the engine is back).
      handleResume();
      // Check for SW updates on resume — iOS PWA never reloads, so this is the
      // only chance to detect new versions after the initial page load.
      navigator.serviceWorker?.getRegistration().then(reg => reg?.update()).catch(() => {});
      // Probe the SW for liveness too — a wedged SW won't accept update()
      // either, so resume is the natural moment to detect and recover.
      checkSwHealth().catch(() => { /* best-effort recovery; next probe retries */ });
    }

    // Closes the gap where focus/visibilitychange triggers don't fire (tab
    // already visible) but the SW has wedged — Chromium issue #370536109.
    // Started/stopped on visibility so hidden tabs don't burn CPU wakes.
    let swProbeInterval: number | null = null;
    function startSwProbe() {
      if (swProbeInterval !== null) return;
      swProbeInterval = window.setInterval(() => {
        checkSwHealth().catch(() => { /* best-effort recovery; next probe retries */ });
      }, SW_PROBE_INTERVAL_MS);
    }
    function stopSwProbe() {
      if (swProbeInterval === null) return;
      clearInterval(swProbeInterval);
      swProbeInterval = null;
    }

    function handleVisibilityChange() {
      if (document.visibilityState === 'visible') {
        onResume();
        startSwProbe();
      } else {
        stopSwProbe();
      }
    }
    document.addEventListener('visibilitychange', handleVisibilityChange);
    window.addEventListener('focus', onResume);
    window.addEventListener('pageshow', onResume);
    if (document.visibilityState === 'visible') startSwProbe();

    // Periodic health polling as connection watchdog.
    const connectionInterval = setInterval(() => {
      checkConnection();
    }, CONNECTION_POLL_INTERVAL);

    // WKWebView crash recovery: send periodic heartbeats to the Tauri Rust side.
    // If heartbeats stop (content process crashed → white screen), the Rust
    // watchdog reloads the webview automatically.
    const heartbeatInterval = isTauri()
      ? setInterval(() => { invoke('heartbeat').catch(() => {}); }, 15_000)
      : null;

    return () => {
      unmounted = true;
      stopLiveness();
      clearInterval(connectionInterval);
      stopSwProbe();
      if (heartbeatInterval) clearInterval(heartbeatInterval);
      clearTimeout(initialHealthCheck);
      window.removeEventListener('message', onAppFrameMessage);
      stopHashRouting();
      navigator.serviceWorker?.removeEventListener('message', onServiceWorkerMessage);
      navigator.serviceWorker?.removeEventListener('controllerchange', requestServiceWorkerBuildId);
      disconnectThreadEvents();
      stopDevicePresence();
      stopScrollVisibility();
      document.removeEventListener('click', onGlobalClick);
      document.removeEventListener('visibilitychange', handleVisibilityChange);
      window.removeEventListener('focus', onResume);
      window.removeEventListener('pageshow', onResume);
    };
  }, []);
}
