import { useEffect } from 'preact/hooks';
import { checkConnection, handleResume } from '../store/actions/connection';
import { loadArtifacts, openUrl } from '../store/actions/artifacts';
import { refreshUnreadCount, viewNotification, loadNotifications } from '../store/actions/notifications';
import { loadApps, openAppById } from '../store/actions/apps';
import { loadCredentials } from '../store/actions/credentials';
import { loadDevices, registerCurrentDevice } from '../store/actions/devices';
import { loadTriggers } from '../store/actions/triggers';
import { loadPreferences } from '../store/actions/preferences';
import { loadPinnedApps } from '../store/actions/pinnedApps';
import { connectThreadEvents, disconnectThreadEvents } from '../store/actions/thread-sync';
import { loadAllThreads } from '../store/actions/thread-loading';
import { refreshPushSubscription } from '../store/actions/push';
import { startPresenceTracking } from '../store/actions/presence';
import { API_BASE } from '../api/client';
import { isTauri } from '../utils/platform';
import { invoke } from '../utils/tauri';
import { refreshChangesState, restoreRestartToast } from '../store/actions/chat-changes';
import { focusThread } from '../store/actions/threads';
import { CHECK_ICON, COPY_ICON } from '../utils/renderMarkdown';
import { activeMenuItem, focusedThreadId, settingsSubview, updateAvailable, threadsLoaded, showToast, showConfirm } from '../store/store';
import { shouldShowSwUpdateToast, markSwUpdateDismissed } from './sw-update';
import { FOCUSED_THREAD_KEY } from '../utils/draftStorage';

const CONNECTION_POLL_INTERVAL = 5000;
const API = `${API_BASE}/api`;

/** Route a notification deep-link to the right action.
 *
 *  Precedence is `thread > app > notification` — a tap that targets a specific
 *  thread (e.g. the seeded "Claude is asking" trigger) opens the conversation
 *  directly; opening the notification modal on top would just hide it.
 *  Returns true when something was dispatched.
 */
function dispatchDeepLink(target: { thread?: string | null; app?: string | null; notification?: string | null }): boolean {
  if (target.thread) { focusThread(target.thread); return true; }
  if (target.app) { openAppById(target.app); return true; }
  if (target.notification) { viewNotification(target.notification); return true; }
  return false;
}

/** Check backend for a pending notification click stored by the SW via fetch().
 *  When includePushFallback is true, also checks /notification-pushed — the
 *  fallback for iOS where notificationclick fires too late or not at all on
 *  warm resume. The push fallback has a 60-second server-side expiry to avoid
 *  showing stale pushes when the user opens the app independently.
 */
async function checkPendingNotification(includePushFallback = false): Promise<boolean> {
  // Primary: notification-clicked (stored by SW notificationclick handler)
  try {
    const res = await fetch(`${API}/notification-clicked`);
    if (res.ok) {
      const data = await res.json();
      if (data.notification_id) {
        viewNotification(data.notification_id);
        return true;
      }
    }
  } catch {}

  // Fallback: notification-pushed (stored by SW push handler at delivery time)
  if (includePushFallback) {
    try {
      const res = await fetch(`${API}/notification-pushed`);
      if (res.ok) {
        const data = await res.json();
        if (data.notification_id) {
          viewNotification(data.notification_id);
          return true;
        }
      }
    } catch {}
  }

  return false;
}

export function useStartup(): void {
  useEffect(() => {
    // Restore focused thread from localStorage (set at signal init, reinforce here)
    const savedThreadId = localStorage.getItem(FOCUSED_THREAD_KEY);
    if (savedThreadId) {
      focusedThreadId.value = savedThreadId;
    }

    // Initial loads
    checkConnection().then((connected) => {
      if (connected) {
        connectThreadEvents();
      }
    });
    refreshUnreadCount();
    loadPreferences().then(() => {
      // Notifications must load after preferences so the persisted filter is applied
      if (activeMenuItem.value === 'notifications') loadNotifications();
    });
    loadPinnedApps();
    loadAllThreads().catch(() => {
      // Retry after 3s — covers transient network failures on initial load.
      // If this also fails, the 5s health poll will keep retrying.
      setTimeout(() => loadAllThreads().catch(() => {}), 3000);
    });
    registerCurrentDevice();
    const stopPresence = startPresenceTracking();
    restoreRestartToast();  // Immediate — show from localStorage before async API
    refreshChangesState();

    // Global click handler for thread links, external URLs, and copy buttons
    function onGlobalClick(e: MouseEvent) {
      const target = e.target as HTMLElement;

      // Thread links
      const threadLink = target.closest('.thread-link') as HTMLElement | null;
      if (threadLink) {
        e.preventDefault();
        const threadId = threadLink.getAttribute('data-thread-id');
        if (threadId) focusThread(threadId);
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

    // Check for push notification deep-link (?notification=<id> / ?app=<id> / ?thread=<id> in URL).
    const params = new URLSearchParams(window.location.search);
    const target = {
      notification: params.get('notification'),
      app: params.get('app'),
      thread: params.get('thread'),
    };
    if (target.notification || target.app || target.thread) {
      // Clear the params from URL so refreshing doesn't re-open
      const url = new URL(window.location.href);
      url.searchParams.delete('notification');
      url.searchParams.delete('app');
      url.searchParams.delete('thread');
      window.history.replaceState({}, '', url.toString());
      // Defer so stores have a tick to hydrate before navigation
      setTimeout(() => dispatchDeepLink(target), 500);
    } else {
      // Check for a pending notification from SW (cold start after notification tap)
      setTimeout(() => checkPendingNotification(true), 500);
    }

    loadArtifacts();
    loadApps();

    // Load data for the restored active menu item (switchMenuItem isn't called on reload)
    const tab = activeMenuItem.value;
    if (tab === 'triggers') loadTriggers();
    if (tab === 'settings') {
      loadDevices();
      if (settingsSubview.value === 'accounts') loadCredentials();
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
      });
    }

    // Handle notification deep-link from SW (via postMessage — instant on Chrome).
    function onSwMessage(event: MessageEvent) {
      if (event.data?.type === 'open-notification') {
        dispatchDeepLink({
          thread: event.data.thread_id,
          app: event.data.app_id,
          notification: event.data.id,
        });
      }
    }
    navigator.serviceWorker?.addEventListener('message', onSwMessage);
    // Start the message queue — without this, messages from client.postMessage()
    // in the SW are buffered indefinitely and never delivered to addEventListener handlers.
    navigator.serviceWorker?.startMessages();

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
      });
    }
    window.addEventListener('message', onAppFrameMessage);

    // On iOS PWA, the page doesn't reload when returning from background.
    // Check backend for pending notification clicks and reconnect SSE.
    // Notification check runs unconditionally (it's a simple GET with no side
    // effects). SSE reconnection and SW update are guarded by threadsLoaded to
    // avoid races during initial page load (startup already handles those).
    function onResume() {
      checkPendingNotification(true);
      if (!threadsLoaded.value) return;
      // handleResume health-checks the engine first. If unreachable, it defers
      // the sync to the 5s health poll (which calls checkConnection, which
      // picks up the deferred resume once the engine is back).
      handleResume();
      // Check for SW updates on resume — iOS PWA never reloads, so this is the
      // only chance to detect new versions after the initial page load.
      navigator.serviceWorker?.getRegistration().then(reg => reg?.update()).catch(() => {});
    }

    function handleVisibilityChange() {
      if (document.visibilityState === 'visible') onResume();
    }
    document.addEventListener('visibilitychange', handleVisibilityChange);
    window.addEventListener('focus', onResume);
    window.addEventListener('pageshow', onResume);

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
      clearInterval(connectionInterval);
      if (heartbeatInterval) clearInterval(heartbeatInterval);
      navigator.serviceWorker?.removeEventListener('message', onSwMessage);
      window.removeEventListener('message', onAppFrameMessage);
      disconnectThreadEvents();
      stopPresence();
      document.removeEventListener('click', onGlobalClick);
      document.removeEventListener('visibilitychange', handleVisibilityChange);
      window.removeEventListener('focus', onResume);
      window.removeEventListener('pageshow', onResume);
    };
  }, []);
}
