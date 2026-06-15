import { API, json, mutatingFetch, throwIfNotOk } from '../../api/client';
import { showToast } from '../store';
import { getDeviceId } from './devices';
import { isTauri, isIOS, isStandalone } from '../../utils/platform';
import { errorDetail } from '../../utils/errorDetail';

/** Convert a base64url-encoded string to a Uint8Array (for applicationServerKey) */
function urlBase64ToUint8Array(base64String: string): Uint8Array {
  const padding = '='.repeat((4 - (base64String.length % 4)) % 4);
  const base64 = (base64String + padding).replace(/-/g, '+').replace(/_/g, '/');
  const rawData = atob(base64);
  const outputArray = new Uint8Array(rawData.length);
  for (let i = 0; i < rawData.length; i++) {
    outputArray[i] = rawData.charCodeAt(i);
  }
  return outputArray;
}

/** Fetch the VAPID public key from the backend. Routed through `json()` so
 *  transport errors surface as `ApiError` (with the engine's `{error}` body)
 *  instead of a generic `TypeError`, matching every other GET in this module. */
async function getVapidKey(): Promise<string> {
  const data = await json<{ public_key: string }>(`${API}/push/vapid-key`);
  return data.public_key;
}

/** Send a push subscription to the backend */
async function subscribePush(subscription: PushSubscription): Promise<void> {
  const json = subscription.toJSON();
  const res = await mutatingFetch(`${API}/push/subscribe`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      endpoint: json.endpoint,
      p256dh: json.keys?.p256dh || '',
      auth: json.keys?.auth || '',
      device_id: getDeviceId(),
    }),
  });
  await throwIfNotOk(res);
}

/** Ask the browser for a push subscription using the engine's current VAPID key. */
async function createPushSubscription(
  registration: ServiceWorkerRegistration,
): Promise<PushSubscription> {
  const vapidKey = await getVapidKey();
  const applicationServerKey = urlBase64ToUint8Array(vapidKey);
  return registration.pushManager.subscribe({
    userVisibleOnly: true,
    applicationServerKey: applicationServerKey.buffer as ArrayBuffer,
  });
}

/**
 * Silently re-subscribe push if already permitted.
 * Called on every page load to keep the endpoint fresh in the backend.
 *
 * Self-heals two divergent states that leave `devices.push_enabled = true`
 * with no `push_subscriptions` row, so the engine pushes to nothing and the
 * user sees silence: (1) the LLM `enable_push_notifications` tool flips the
 * device flag before the browser handshake completes; (2) the browser loses
 * the subscription (cleared site data, SW unregistered). In either case
 * `Notification.permission` stays 'granted', so we can `subscribe()` silently
 * without a permission prompt.
 */
export async function refreshPushSubscription(registration: ServiceWorkerRegistration): Promise<void> {
  try {
    if (isTauri()) return;
    if (!('PushManager' in window)) return;
    if (Notification.permission !== 'granted') return;

    const existing = await registration.pushManager.getSubscription();
    if (existing) {
      // Re-send the current subscription to the backend (updates device_id, replaces stale rows)
      await subscribePush(existing);
      return;
    }

    // pushManager.subscribe() doesn't prompt when permission is already 'granted'.
    await subscribePush(await createPushSubscription(registration));
  } catch (err) {
    // Telemetry carve-out (.claude/rules/frontend.md): background refresh
    // runs on every page load without user intent. Failures are non-blocking;
    // if the endpoint is stale, the next push attempt re-triggers the
    // user-facing initPushSubscription() flow which toasts on its own.
    console.warn('[Push] Failed to refresh subscription:', err);
  }
}

/**
 * Force a fresh service worker by unregistering the existing one and
 * registering again. Used to recover from a "wedged" SW state where Chrome
 * can no longer deliver events to it — the symptom users see is notification
 * clicks doing nothing even though `event.notification.close()` is the
 * first statement in the handler. The page detects this by sending a ping
 * the SW would normally pong; no pong → call this. Equivalent to the
 * user restarting Chrome (closes redundant SW state, fresh install + activate).
 *
 * unregister() invalidates the push subscription as a side effect, so the
 * resubscribe at the end keeps push delivery alive across the recovery.
 */
export async function recoverServiceWorker(): Promise<void> {
  if (!('serviceWorker' in navigator)) return;
  const existing = await navigator.serviceWorker.getRegistration();
  if (existing) {
    // Notifications shown by this registration would be orphaned once we
    // unregister: Chrome keeps them visible but routes their click events
    // to a registration that no longer exists, so the click does nothing
    // (manifests as a "stuck" notification you can't click, only manually
    // dismiss in the Chrome notification UI). Dismiss them on-screen now;
    // the unread state is durable in the backend and the user still sees
    // every notification in the in-app inbox.
    try {
      const stale = await existing.getNotifications();
      for (const n of stale) n.close();
    } catch {
      /* best-effort orphan cleanup — don't block the recovery on it */
    }
    await existing.unregister();
  }
  await navigator.serviceWorker.register('/sw.js', { updateViaCache: 'none' });
  // pushManager.subscribe requires an active worker; calling pre-activation
  // rejects with InvalidStateError. `ready` resolves to the registration that
  // owns the active worker — passing the post-register registration directly
  // can hand refreshPushSubscription one still in 'installing'.
  const active = await navigator.serviceWorker.ready;
  await refreshPushSubscription(active);
}

/**
 * Register the service worker, request notification permission,
 * subscribe to push, and send the subscription to the backend.
 *
 * Called when the frontend receives a `push_notification_request` SSE event.
 */
export async function initPushSubscription(): Promise<boolean> {
  if (isTauri()) {
    // Desktop gets NATIVE macOS notifications driven by the engine's
    // `NativePushRequested` SSE (rendered + tap-routed via the
    // `show_native_notification` command). There's no web-push subscription to
    // create — the WKWebView can't subscribe — and the macOS notification path
    // has no JS-queryable permission (banners follow the user's System Settings,
    // and macOS prompts on first delivery), so enabling is a no-op success.
    // See system-knowhow/notifications.md §4.
    showToast('Notifications enabled', 'success');
    return true;
  }

  if (isIOS() && !isStandalone()) {
    showToast('On iOS, add Lucidos to your home screen first to enable push notifications', 'error');
    return false;
  }

  if (!('serviceWorker' in navigator) || !('PushManager' in window)) {
    showToast('Push notifications are not supported in this browser', 'error');
    return false;
  }

  if (!('Notification' in window)) {
    showToast('Notifications are not supported in this browser', 'error');
    return false;
  }

  try {
    const registration = await navigator.serviceWorker.register('/sw.js');

    const permission = await Notification.requestPermission();
    if (permission !== 'granted') {
      showToast('Notification permission was denied', 'error');
      return false;
    }

    await subscribePush(await createPushSubscription(registration));
    showToast('Push notifications enabled', 'success');
    return true;
  } catch (err) {
    showToast(`Failed to enable push notifications: ${errorDetail(err)}`, 'error');
    return false;
  }
}

