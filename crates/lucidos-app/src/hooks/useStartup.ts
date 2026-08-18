import { useEffect } from 'preact/hooks';
import { checkConnection, handleResume, bounceToPickerIfStranded } from '../store/actions/connection';
import { loadArtifacts, openUrl } from '../store/actions/artifacts';
import { openFilePreviewModal, filePreviewRequestError, filePreviewBlockedReason } from '../store/actions/filePreviewModal';
import { syncAppFullscreenHost } from '../store/appFullscreenHost';
import { loadUnreadNotifications, loadNotifications } from '../store/actions/notifications';
import { syncWorkspaceAppBadge, refreshOtherWorkspacesUnread } from '../store/actions/app-badge';
import { loadApps } from '../store/actions/apps';
import { loadCredentials } from '../store/actions/credentials';
import { loadDevices, registerCurrentDevice } from '../store/actions/devices';
import { loadTriggers, loadHistoricalTriggers } from '../store/actions/triggers';
import { loadTriggerGroups } from '../store/actions/triggerGroups';
import { loadThreadQueue } from '../store/actions/threadQueue';
import { loadRepositories } from '../store/actions/chat';
import { loadPreferences, flushPendingPreferenceWrites } from '../store/actions/preferences';
import { loadPinnedApps } from '../store/actions/pinnedApps';
import { loadWorkspaceDisplayName } from '../store/actions/workspace-label';
import { connectThreadEvents, disconnectThreadEvents } from '../store/actions/thread-sync';
import { loadAllThreads, loadFilterFacets } from '../store/actions/thread-loading';
import { refreshPushSubscription, recoverServiceWorker } from '../store/actions/push';
import { setupNativePushTapRouting } from '../store/actions/native-push';
import { startDevicePresenceTracking } from '../store/actions/device-presence';
import { startAppUpdateChecks, stopAppUpdateChecks, recheckAppUpdateOnResume } from '../store/actions/app-update';
import { startEngineUpdateChecks, stopEngineUpdateChecks, checkEngineVersion } from '../store/actions/engine-update';
import {
  loadEmbeddingModelStatus,
  subscribeToTailscaleServeProgress,
  unsubscribeFromTailscaleServeProgress,
} from '../store/actions/backgroundActivity';
import { isTauri } from '../utils/platform';
import { invoke } from '../utils/tauri';
import { refreshChangesState, restoreRestartState } from '../store/actions/chat-changes';
import { restoreRepoSelectionFromStorage } from '../store/actions/repositories';
import { openThreadAcrossWorkspaces } from '../store/actions/cross-workspace';
import { CHECK_ICON, COPY_ICON } from '../utils/markedConfig';
import { activeMenuItem, notificationsFilter, settingsSubview, serviceWorkerBuildId, threadsLoaded, showToast, showConfirm, showPrompt, CONNECTION_POLL_INTERVAL_MS, FOCUSED_THREAD_KEY, setFocusedThread } from '../store/store';
import { installContentPaneIframeFocusTracking } from '../components/layout/paneFocus';
import { requestServiceWorkerBuildId } from './sw-update';
import { syncClientUpdateFromBuild } from '../store/actions/client-update';
import {
  parseDeepLinkFromSwMessage,
  hasDeepLinkParams,
} from '../store/actions/notification-deeplink';
import { dispatchDeepLink } from '../store/actions/in-app-notification-toast';
import { setupHashDeeplinkRouting } from '../store/actions/hash-deeplink-router';
import { reportStartupKind, startLivenessTracking } from '../utils/liveness';
import { createLeadingEdgeGate } from '../utils/leadingEdgeGate';
import { flushUndeliveredComposeDrafts } from '../store/actions/compose';
import { isKnownAppFrame } from '../utils/appFrame';
import { handleAppToastMessage } from '../store/actions/app-toast-bridge';
import { withBase, SCOPE_PATH } from '../utils/basePath';
import { isDevServerBundle, DEV_SERVER_SW_REASON } from '../utils/devServerBundle';

// Cold-start recovery window: if the very first connect hasn't landed by this
// point, return to the workspace picker rather than strand the user in a cached
// shell that can't load (the PWA auto-open-into-an-unreachable-engine case).
// ~2 health polls (immediate + 5s) plus slack. No-op once connected; the bounce
// itself is one-shot guarded in connection.ts.
const COLD_START_BOUNCE_MS = 10_000;
// Catches Chrome's idle-SW LRU eviction (Chromium issue #370536109:
// notificationclick silently dropped) without churning the recovery path,
// which is itself cooldown-debounced.
const SW_PROBE_INTERVAL_MS = 5 * 60 * 1000;
// How often a gateway-served page re-reads the OTHER workspaces' unread counts
// for the app-icon badge (one installed icon covers the whole gateway origin,
// see store/actions/app-badge.ts). Deliberately slow: a notification in another
// workspace already repaints the icon through its own push, which carries the
// fresh aggregate, so this is the backstop for the case with no signal at all
// (a notification READ in another workspace, on another device). A no-op
// outside a gateway workspace context.
const CROSS_WORKSPACE_BADGE_INTERVAL_MS = 60 * 1000;
// One iOS wake delivers `visibilitychange`, `focus` and `pageshow` together, so
// the resume fan-out is gated to one pass per window. Only has to outlast the
// burst (same tick, occasionally a few hundred ms apart); a genuine later wake
// still gets its own pass. See `onResumeCoalesced`.
const RESUME_COALESCE_MS = 1000;

/** A `setInterval` that only ticks while the document is visible: `start()` on
 *  visibility-visible, `stop()` when hidden, so a backgrounded tab burns no
 *  wakes on work nobody is looking at. Both are idempotent, so the visibility
 *  handler can call them unconditionally, and `stop()` doubles as teardown. */
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

    // Restore the restart state from localStorage BEFORE the first
    // checkConnection: a reload mid-restart re-raises the progress dialog and
    // seeds engineRestarting + the pre-restart started_at, which checkConnection
    // reads to detect the restart completion. (Cold start with only a pending
    // restart re-arms the brand badge and the restart confirm dialog.)
    restoreRestartState();  // Immediate: from localStorage, before the async API

    // Initial loads
    checkConnection().then((connected) => {
      if (connected) {
        connectThreadEvents();
      }
    }).catch(() => { /* checkConnection swallows internally; satisfy fail-fast rule */ });
    // Cold-start: load the unread set so the bell badge is correct.
    void loadUnreadNotifications();
    // ...and the other workspaces' counts, which the bell never shows but the
    // app ICON does: behind the gateway one installed icon covers every
    // workspace on the origin. A no-op on a direct engine port. See
    // store/actions/app-badge.ts.
    void refreshOtherWorkspacesUnread();
    loadPreferences().then(() => {
      // Notifications must load after preferences so the persisted filter is
      // applied. The "Unread" tab renders `unreadNotifications` (loaded above via
      // loadUnreadNotifications), so only the "All" tab needs the paginated
      // browse list loaded here.
      if (activeMenuItem.value === 'notifications' && notificationsFilter.value === 'all') {
        loadNotifications();
      }
      // The version-update dismissals are now GLOBAL preferences, so the update
      // surfaces skip while preferences are still loading (they can't yet know if
      // this build was dismissed). Re-derive now that preferences are known: the
      // client-refresh has no poll of its own, so without this a previously
      // dismissed toast would flash on cold start and linger until the next resume
      // / PreferencesChanged; the engine-switch also derives promptly instead of
      // waiting up to one 4s poll.
      void syncClientUpdateFromBuild();
      void checkEngineVersion();
    }).catch(() => { /* loadPreferences sets Loadable failed internally — UI shows the error */ });
    void loadPinnedApps();
    // The name the user gave this workspace lives in the gateway registry, not
    // in the engine, so ask for it (see actions/workspace-label.ts). Until it
    // lands, every surface shows the engine's own name.
    void loadWorkspaceDisplayName();
    // Snapshot the embedding-model load. On a fresh workspace the ~465 MB
    // download starts at engine boot, seconds before this document exists, so
    // every SSE frame so far has already been missed; without this read the
    // status toast would not open until the next one arrives, and a warm
    // workspace would never learn the model is ready.
    void loadEmbeddingModelStatus();
    // An Expose run narrates itself over a Tauri event, and it outlives the pane
    // that started it, so the listener belongs at startup beside the updater's
    // rather than on the Mobile Access page.
    void subscribeToTailscaleServeProgress();
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
    // Route native macOS notification taps (Tauri only) through the same
    // dispatchDeepLink the web-push tap uses. No-op listener off-Tauri.
    let stopNativeTap: (() => void) | null = null;
    let nativeTapCanceled = false;
    setupNativePushTapRouting()
      .then((un) => {
        if (nativeTapCanceled) un();
        else stopNativeTap = un;
      })
      .catch(() => { /* best-effort: tap routing is additive; banners still show */ });
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
    loadThreadQueue();
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
    // Captured once `serviceWorker.ready` resolves so the cleanup can detach the
    // `updatefound` listener symmetrically with every other listener below — an
    // HMR remount would otherwise stack a duplicate on the same registration.
    // `onUpdateFound` is block-scoped to the `if` below, so the handler ref is
    // hoisted here for the cleanup to reach.
    let updateFoundReg: ServiceWorkerRegistration | null = null;
    let updateFoundHandler: ((this: ServiceWorkerRegistration) => void) | null = null;
    if (!window.isSecureContext && !isTauri()) {
      // Telemetry carve-out (.claude/rules/frontend.md): runs on every page load
      // without user intent, so no toast (a per-load nag can't be dismissed for
      // good). On an insecure origin (plain http://<host> — e.g. a packaged
      // Linux install reached over the LAN) the platform grants no service
      // worker and no push; Chrome hides navigator.serviceWorker entirely, so
      // the block below silently no-ops. The user still finds out where it
      // matters: enabling push (Settings → Devices) toasts the same condition
      // via pushUnsupportedReasonHere().
      console.warn(
        `[Startup] Insecure origin (${location.origin}): service worker + push notifications unavailable. Open Lucidos over https:// or localhost (SSH tunnel / tailscale serve).`,
      );
    }
    if (isDevServerBundle()) {
      // Telemetry carve-out (.claude/rules/frontend.md): runs on every page load
      // with no user intent, so no toast. A Vite dev server serves unhashed
      // module URLs and an unstamped sw.js, so a worker here would cache the
      // preview's own modules and defeat hot reload (utils/devServerBundle.ts).
      // The user still finds out where it matters: enabling push reports the
      // same condition through pushUnsupportedReasonHere().
      console.warn(`[Startup] ${DEV_SERVER_SW_REASON}`);
    } else if ('serviceWorker' in navigator) {
      // Base-path aware (ADR 0014): behind the gateway the SW is served at
      // /<slug>/sw.js and scoped to /<slug>/, so each workspace is an
      // independent PWA cache + push scope.
      navigator.serviceWorker.register(withBase('/sw.js'), { scope: SCOPE_PATH, updateViaCache: 'none' })
        .then((reg) => {
          refreshPushSubscription(reg);
        })
        .catch(() => showToast('Service worker registration failed — push notifications may not work', 'error'));

      // When the browser activates a new service worker, surface the badge + the
      // "New version available" toast through the SINGLE reliable build-id check
      // (syncClientUpdateFromBuild) rather than deciding here. The check compares
      // the LOADED bundle against the served /sw.js, so it's correct regardless
      // of whether this worker has claimed the page yet, and it can't disagree
      // with the badge (which is driven by the same check on startup/resume).
      // This same path also fires via controllerchange below — both routes are
      // idempotent (the toast is keyed + dedup-guarded). A fresh first install is
      // naturally a no-op: CLIENT_BUILD_ID equals the served id it just loaded.
      function onUpdateFound(this: ServiceWorkerRegistration) {
        const newWorker = this.installing;
        if (!newWorker) return;
        newWorker.addEventListener('statechange', () => {
          if (newWorker.state === 'activated') void syncClientUpdateFromBuild();
        });
      }
      updateFoundHandler = onUpdateFound;
      navigator.serviceWorker.ready.then(reg => {
        if (unmounted) return; // a remount (HMR) tore us down before ready resolved
        updateFoundReg = reg;
        reg.addEventListener('updatefound', onUpdateFound);
      }).catch(() => { /* SW not ready in this environment — update toast is best-effort */ });

      // Listens for two SW-originated messages: the deep-link delivery (how a
      // notification tap routes an already-open tab — routeToDeepLink in sw.js
      // posts the structured deep link rather than fragment-navigating; see
      // system-knowhow/notifications.md §4.5) and the liveness pong.
      navigator.serviceWorker.addEventListener('message', onServiceWorkerMessage);

      // Surface the active SW's BUILD_ID on the System page (debugging aid for
      // "did the new build's SW take over?"). Query now, and again whenever a new
      // SW claims the page so the shown id tracks the live worker.
      requestServiceWorkerBuildId();
      navigator.serviceWorker.addEventListener('controllerchange', requestServiceWorkerBuildId);

      // Light/clear the update badge for THIS load by comparing the running
      // bundle's build id against the served sw.js. Independent of the SW
      // build-id reply above (that drives the "Service worker" debug row) — this
      // is the honest "is my loaded code stale?" check, also re-run on resume.
      void syncClientUpdateFromBuild();
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
        if (typeof data.buildId === 'string') {
          serviceWorkerBuildId.value = data.buildId; // System page "Service worker" row
          // A reply also arrives on controllerchange (a SW swap), so re-check
          // whether the running bundle is now stale vs the served build. The
          // check itself reads CLIENT_BUILD_ID, not this controller id.
          void syncClientUpdateFromBuild();
        }
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
        payload?: {
          title?: unknown; message?: unknown; okLabel?: unknown; cancelLabel?: unknown; danger?: unknown;
          type?: unknown; durationMs?: unknown; dismissable?: unknown; key?: unknown; spinning?: unknown;
          defaultValue?: unknown; placeholder?: unknown; multiline?: unknown;
          file_path?: unknown; line?: unknown; line_end?: unknown;
        };
      } | null;
      if (!data || typeof data !== 'object') return;
      if (
        data.type !== 'lucidos:ui:confirm' && data.type !== 'lucidos:ui:toast'
        && data.type !== 'lucidos:ui:dismissToast'
        && data.type !== 'lucidos:ui:prompt' && data.type !== 'lucidos:ui:preview-file'
      ) return;
      const payload = data.payload;
      if (!payload || typeof payload !== 'object') return;

      // Reject messages from any iframe that isn't a current app iframe,
      // so nested iframes (embeds, ads) can't trigger host modals / toasts.
      // Ahead of every branch below, deliberately: a frame we don't know gets no
      // host chrome and no reply, whatever it asked for.
      const source = event.source as Window | null;
      if (!source || !isKnownAppFrame(source)) return;

      // File preview: a read-only modal over the app, carrying a locator rather
      // than a message. Answered as soon as we have decided, since the SDK's
      // promise resolves when the preview is showing (not when it is dismissed).
      if (data.type === 'lucidos:ui:preview-file') {
        if (typeof data.id !== 'string') return;
        // Re-derive where host chrome renders before deciding: the refusal below
        // and the portal that would act on it must be reading the same instant,
        // and the layer's target is published from a component render, which can
        // lag a fullscreen transition by a frame.
        syncAppFullscreenHost();
        // The request itself, then whether the host can put anything on screen
        // at all (it cannot render over a fullscreen element it does not own).
        // A refusal is the honest answer: a resolved promise with no visible
        // modal is what made this fail silently.
        const error = filePreviewRequestError(payload) ?? filePreviewBlockedReason();
        if (!error) {
          openFilePreviewModal({
            file_path: payload.file_path as string,
            line: payload.line,
            line_end: payload.line_end,
          });
        }
        try {
          source.postMessage(
            { type: 'lucidos:ui:preview-file:result', id: data.id, ok: error === null, error: error ?? undefined },
            '*',
          );
        } catch {
          // Source iframe may have unloaded, so drop the reply silently.
        }
        return;
      }

      // Toast and its dismissal: fire-and-forget, no id, no result reply. Ahead
      // of the message guard below deliberately, since a dismissal carries only
      // a key and that guard would swallow it.
      if (handleAppToastMessage(data.type, payload)) return;

      // Confirm and prompt both carry a message and are useless without one.
      if (typeof payload.message !== 'string' || payload.message.length === 0) return;

      // Both confirm and prompt carry an id and post a result back.
      if (typeof data.id !== 'string') return;
      const title = typeof payload.title === 'string' ? payload.title : undefined;
      const cancelLabel = typeof payload.cancelLabel === 'string' && payload.cancelLabel.length > 0 ? payload.cancelLabel : 'Cancel';

      // Prompt — text input; resolves a string (OK) or null (cancel).
      if (data.type === 'lucidos:ui:prompt') {
        const okLabel = typeof payload.okLabel === 'string' && payload.okLabel.length > 0 ? payload.okLabel : 'OK';
        showPrompt(payload.message, {
          title,
          cancelLabel,
          okLabel,
          defaultValue: typeof payload.defaultValue === 'string' ? payload.defaultValue : undefined,
          placeholder: typeof payload.placeholder === 'string' ? payload.placeholder : undefined,
          multiline: payload.multiline === true,
        }).then((value) => {
          try {
            source.postMessage({ type: 'lucidos:ui:prompt:result', id: data.id, value }, '*');
          } catch {
            // Source iframe may have unloaded — drop the reply silently.
          }
        }).catch(() => { /* showPrompt rejection — drop, modal already closed */ });
        return;
      }

      // Confirm — boolean result.
      const okLabel = typeof payload.okLabel === 'string' && payload.okLabel.length > 0 ? payload.okLabel : 'Confirm';
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
      // The app-icon badge was writable by the OS while we were away (iOS sets
      // it from the push payload's `app_badge`; the SW sets it on Chrome), so
      // re-assert ours the moment we're back — before the reload, so the icon
      // agrees with the bell even if the engine is unreachable.
      syncWorkspaceAppBadge();
      // Reload the unread set on resume so the bell badge reflects anything that
      // arrived while backgrounded (the load re-asserts the icon again from the
      // fresh set). The OS push tap delivers the deep link itself
      // (absolute-navigate URL on iOS, notificationclick elsewhere), so there's
      // no in-app rescue toast to surface here.
      void loadUnreadNotifications();
      // The icon's cross-workspace half went unwatched while we slept (the
      // interval is visible-only, and another workspace's notifications never
      // reach this page's SSE), so re-read it now rather than at the next tick.
      void refreshOtherWorkspacesUnread();
      // Re-send any preference write the engine never received. WebKit aborts
      // in-flight fetches when it suspends the page, so a settings change made
      // just before backgrounding is applied on this device but missing on the
      // server; resume is the first moment it can land. No-op when nothing is
      // parked. See store/actions/preferences.ts.
      void flushPendingPreferenceWrites();
      // Check for SW updates on resume — iOS PWA never reloads, so this is the
      // only chance to detect new versions after the initial page load.
      navigator.serviceWorker?.getRegistration().then(reg => reg?.update()).catch(() => {});
      // Also re-check the BUILD_ID directly: a newer frontend build that landed
      // while we were backgrounded lights the update badge even if the browser's
      // own SW update check is slow/wedged on iOS.
      void syncClientUpdateFromBuild();
      // Reconcile the engine build state too: a PWA suspended for the whole
      // background build missed the transient EngineBuildStateChanged pokes (SSE
      // isn't replayed on reconnect), so re-poll authoritatively on resume — shows
      // 'ready' if the build finished while away, never a stale spin.
      void checkEngineVersion();
      // Same reconciliation for the PACKAGED app release, which this handler used
      // to be the only update surface to skip: a desktop client is long-resident
      // and rarely remounts, so without a resume check it kept reporting itself
      // current for the whole poll interval after a release. Throttled inside
      // (window focus fires constantly); a no-op outside the Tauri client.
      void recheckAppUpdateOnResume();
      // And the embedding model, for the same reason as the engine build: a
      // suspended PWA missed every transient status frame, so re-read the
      // snapshot rather than trusting the last one seen before backgrounding.
      void loadEmbeddingModelStatus();
      // And the name the user gave this workspace, which they may have changed
      // in the picker on another device while this one slept. Behind the
      // gateway the in-app switcher re-adopts on every unfold, so this is
      // belt-and-braces there; on a direct engine port it is the ONLY thing
      // that can correct the name, because the switcher gates itself off (no
      // control plane on that origin) and the "next load" the label otherwise
      // waits for is the load this whole handler exists because iOS never
      // does. See store/actions/workspace-label.ts.
      void loadWorkspaceDisplayName();
      // Re-send any compose draft the engine never received, for the same
      // reason as the preference writes above. A draft lives only in memory
      // (`store/composeDrafts.ts`), so the server is its storage and an
      // undelivered one dies with the next iOS eviction. No-op when nothing is
      // parked. See store/actions/compose.ts.
      flushUndeliveredComposeDrafts();
      // Probe the SW for liveness too — a wedged SW won't accept update()
      // either, so resume is the natural moment to detect and recover.
      checkSwHealth().catch(() => { /* best-effort recovery; next probe retries */ });
    }

    /** One wake, one pass. iOS fires `visibilitychange`, `focus` AND `pageshow`
     *  together on a resume, so every listener below used to run the whole
     *  `onResume` fan-out, three times over: the gateway log showed 3x
     *  `engine/version-status`, 3x `memory/embedding-model-status` and 3-4x
     *  `notifications` inside one second. Two things went wrong with that. The
     *  burst is fired down a tunnel that is itself still re-establishing after
     *  the wake, and it grows with the workspace (85 thread-event GETs in one
     *  minute against 16 earlier the same day). And it silently collapsed the
     *  tolerance of every consecutive-failure counter reached from here:
     *  `loadUnreadNotifications` is meant to stay quiet until three failures in
     *  a row, which ONE bad wake was spending on its own.
     *
     *  Leading edge, so the work is deduplicated and never delayed. The window
     *  only has to outlast the burst; a genuine later wake still gets a full
     *  pass. `handleResume`'s own `resumeInFlight` guard is complementary, not
     *  redundant: it covers a slow in-flight health check that outlives this
     *  window. See `docs/plans/2026-08-03-ios-pwa-resume-storm-and-durable-compose-drafts.md`. */
    const resumeGate = createLeadingEdgeGate(RESUME_COALESCE_MS);
    function onResumeCoalesced() {
      if (!resumeGate.allow()) return;
      onResume();
    }

    // Closes the gap where focus/visibilitychange triggers don't fire (tab
    // already visible) but the SW has wedged — Chromium issue #370536109.
    const swProbe = visibleOnlyInterval(() => {
      checkSwHealth().catch(() => { /* best-effort recovery; next probe retries */ });
    }, SW_PROBE_INTERVAL_MS);

    // Keeps the app-icon badge's cross-workspace half honest while the page sits
    // open (see CROSS_WORKSPACE_BADGE_INTERVAL_MS).
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

    // Periodic health polling as connection watchdog.
    const connectionInterval = setInterval(() => {
      checkConnection();
    }, CONNECTION_POLL_INTERVAL_MS);

    // Cold-start recovery: if we never reach the engine, bounce back to the
    // workspace picker (the always-reachable recovery surface). No-op once
    // connected / with no picker / after a prior bounce. See connection.ts.
    const coldStartBounceTimer = window.setTimeout(() => {
      bounceToPickerIfStranded();
    }, COLD_START_BOUNCE_MS);

    // WKWebView crash recovery: send periodic heartbeats to the Tauri Rust side.
    // If heartbeats stop (content process crashed → white screen), the Rust
    // watchdog reloads the webview automatically.
    //
    // The `catch` is a local no-op, not a swallowed signal: `invoke` reports
    // every failure to the engine log itself (utils/ipcHealth → `[Client/ipc]`).
    // There is nothing useful to do here per-beat — the next beat is 15s away
    // and the Rust watchdog is the recovery path — and a toast would be wrong on
    // a timer the user never started.
    const heartbeatInterval = isTauri()
      ? setInterval(() => { invoke('heartbeat').catch(() => {}); }, 15_000)
      : null;

    // Packaged build: surface "update available" INSIDE the workspace (in-app
    // toast), checking on startup + on an interval so a long-resident client still
    // notices. Tauri-only; a no-op in a browser/PWA/dev. See store/actions/app-update.ts.
    startAppUpdateChecks();

    // Dev: poll the engine version-status so a background rebuild (kicked off by
    // Apply) surfaces "New version available → Switch to new version" once ready.
    // No-op in packaged (that path uses the release updater above). See
    // store/actions/engine-update.ts.
    startEngineUpdateChecks();

    return () => {
      unmounted = true;
      stopLiveness();
      clearInterval(connectionInterval);
      clearTimeout(coldStartBounceTimer);
      swProbe.stop();
      crossWorkspaceBadge.stop();
      if (heartbeatInterval) clearInterval(heartbeatInterval);
      stopAppUpdateChecks();
      unsubscribeFromTailscaleServeProgress();
      stopEngineUpdateChecks();
      clearTimeout(initialHealthCheck);
      window.removeEventListener('message', onAppFrameMessage);
      stopHashRouting();
      navigator.serviceWorker?.removeEventListener('message', onServiceWorkerMessage);
      navigator.serviceWorker?.removeEventListener('controllerchange', requestServiceWorkerBuildId);
      if (updateFoundReg && updateFoundHandler) {
        updateFoundReg.removeEventListener('updatefound', updateFoundHandler);
      }
      disconnectThreadEvents();
      stopDevicePresence();
      nativeTapCanceled = true;
      stopNativeTap?.();
      document.removeEventListener('click', onGlobalClick);
      document.removeEventListener('visibilitychange', handleVisibilityChange);
      window.removeEventListener('focus', onResumeCoalesced);
      window.removeEventListener('pageshow', onResumeCoalesced);
    };
  }, []);

  // Keep the content-pane focus marker fresh when keyboard focus lands inside a
  // content-pane iframe (app, file/HTML/PDF preview, cross-origin URL preview).
  // Those clicks never reach the pane's own onPointerDown handler, so the focus
  // marker would otherwise go stale. See installContentPaneIframeFocusTracking
  // (paneFocus.ts).
  useEffect(() => installContentPaneIframeFocusTracking(), []);
}
