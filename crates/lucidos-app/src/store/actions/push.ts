import { API_BASE } from '../../api/client';
import { showToast } from '../store';
import { getDeviceId } from './devices';
import { isTauri, isIOS, isStandalone } from '../../utils/platform';

const API = `${API_BASE}/api`;

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

/** Fetch the VAPID public key from the backend */
async function getVapidKey(): Promise<string> {
  const res = await fetch(`${API}/push/vapid-key`);
  if (!res.ok) throw new Error(`Failed to get VAPID key: ${res.status}`);
  const data = await res.json();
  return data.public_key;
}

/** Send a push subscription to the backend */
async function subscribePush(subscription: PushSubscription): Promise<void> {
  const json = subscription.toJSON();
  const res = await fetch(`${API}/push/subscribe`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      endpoint: json.endpoint,
      p256dh: json.keys?.p256dh || '',
      auth: json.keys?.auth || '',
      device_id: getDeviceId(),
    }),
  });
  if (!res.ok) throw new Error(`Failed to store subscription: ${res.status}`);
}

/**
 * Silently re-subscribe push if already permitted.
 * Called on every page load to keep the endpoint fresh in the backend.
 */
export async function refreshPushSubscription(registration: ServiceWorkerRegistration): Promise<void> {
  try {
    if (isTauri()) return;
    if (!('PushManager' in window)) return;
    if (Notification.permission !== 'granted') return;

    const existing = await registration.pushManager.getSubscription();
    if (!existing) return; // No subscription — user hasn't opted in

    // Re-send the current subscription to the backend (updates device_id, replaces stale rows)
    await subscribePush(existing);
  } catch (err) {
    console.error('[Push] Failed to refresh subscription:', err);
  }
}

/**
 * Register the service worker, request notification permission,
 * subscribe to push, and send the subscription to the backend.
 *
 * Called when the frontend receives a `push_notification_request` SSE event.
 */
export async function initPushSubscription(): Promise<boolean> {
  if (isTauri()) {
    showToast('Push notifications are not available in the desktop app', 'error');
    return false;
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

    const vapidKey = await getVapidKey();
    const applicationServerKey = urlBase64ToUint8Array(vapidKey);

    const subscription = await registration.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: applicationServerKey.buffer as ArrayBuffer,
    });

    await subscribePush(subscription);
    showToast('Push notifications enabled', 'success');
    return true;
  } catch (err) {
    console.error('[Push] Failed to set up push notifications:', err);
    showToast(`Failed to enable push notifications: ${err}`, 'error');
    return false;
  }
}
