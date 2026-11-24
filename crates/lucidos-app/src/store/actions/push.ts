import { noteCrashWorker } from '../../utils/crashWorker';
import { API, json, mutatingFetch, throwIfNotOk, throwIfRefused } from '../../api/client';
import { showToast, showConfirm } from '../store';
import { devices, getDeviceId, toggleDevicePush, disablePushForDevices } from './devices';
import { isTauri, isIOS, isStandalone, isMobileDeviceUserAgent, thisDeviceIsMobile } from '../../utils/platform';
import { deviceName } from '../../utils/deviceFriendlyName';
import { pairedDevices, pairedRows } from './pairedDevices';
import { errorDetail } from '../../utils/errorDetail';
import { SCOPE_PATH, WORKSPACE_ID } from '../../utils/basePath';
import { workspaceServiceWorkerUrl } from '../../utils/workspaceServiceWorker';
import { getGatewayTls } from '../../api/client/control';
import { isDevServerBundle, DEV_SERVER_SW_REASON } from '../../utils/devServerBundle';
import type { DeviceInfo } from '../../api/types';

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

async function getVapidKey(): Promise<string> {
  const data = await json<{ public_key: string }>(`${API}/push/vapid-key`);
  return data.public_key;
}

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
      scope_url: new URL(SCOPE_PATH, window.location.origin).toString(),
    }),
  });
  await throwIfNotOk(res);
  const result = (await res.json()) as { success: boolean; error?: string };
  throwIfRefused(result, 'the engine refused the push subscription');
}

function subscriptionUsesVapidKey(sub: PushSubscription, vapidKey: Uint8Array): boolean | null {
  const raw = sub.options?.applicationServerKey;
  if (!raw) return null;
  const bytes = new Uint8Array(raw);
  return bytes.length === vapidKey.length && bytes.every((b, i) => b === vapidKey[i]);
}

const STALE_KEY_ERROR = 'InvalidStateError';

async function ensurePushSubscription(
  registration: ServiceWorkerRegistration,
): Promise<PushSubscription> {
  const vapidKey = urlBase64ToUint8Array(await getVapidKey());
  const subscribe = () =>
    registration.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: vapidKey.buffer as ArrayBuffer,
    });

  const existing = await registration.pushManager.getSubscription();
  if (existing) {
    const usesCurrentKey = subscriptionUsesVapidKey(existing, vapidKey);
    if (usesCurrentKey) return existing;
    if (usesCurrentKey === false) {
      await existing.unsubscribe();
      return subscribe();
    }
  }

  try {
    return await subscribe();
  } catch (err) {
    if (!existing || (err as { name?: string })?.name !== STALE_KEY_ERROR) throw err;
    await existing.unsubscribe();
    return subscribe();
  }
}

export async function refreshPushSubscription(registration: ServiceWorkerRegistration): Promise<void> {
  try {
    if (isTauri()) return;
    if (!('PushManager' in window)) return;
    if (Notification.permission !== 'granted') return;

    await subscribePush(await ensurePushSubscription(registration));
  } catch (err) {
    // BEST-EFFORT: telemetry carve-out (.claude/rules/frontend.md). The background
    // refresh runs on every page load without user intent.
    // Failures are non-blocking;
    // if the endpoint is stale, the next push attempt re-triggers the
    // user-facing initPushSubscription() flow which toasts on its own.
    console.warn('[Push] Failed to refresh subscription:', err);
  }
}

export async function unregisterServiceWorker(): Promise<void> {
  const existing = await navigator.serviceWorker.getRegistration();
  if (!existing) return;
  try {
    const stale = await existing.getNotifications();
    for (const n of stale) n.close();
  } catch {
  }
  noteCrashWorker(6);
  await existing.unregister();
}

export async function recoverServiceWorker(): Promise<void> {
  if (!('serviceWorker' in navigator)) return;
  if (isDevServerBundle()) return;
  await unregisterServiceWorker();
  await navigator.serviceWorker.register(workspaceServiceWorkerUrl(), { scope: SCOPE_PATH, updateViaCache: 'none' });
  const active = await navigator.serviceWorker.ready;
  await refreshPushSubscription(active);
}

export function pushUnsupportedReason(ctx: {
  secureContext: boolean;
  hasServiceWorker: boolean;
  hasPushManager: boolean;
  hasNotification: boolean;
  devServerBundle: boolean;
  httpsOrigin?: string | null;
}): string | null {
  if (ctx.devServerBundle) {
    return DEV_SERVER_SW_REASON;
  }
  if (!ctx.secureContext && ctx.httpsOrigin) {
    return `Push needs a secure origin. Open Lucidos at ${ctx.httpsOrigin}, which this gateway serves over HTTPS.`;
  }
  if (!ctx.secureContext) {
    return 'Push needs a secure origin (https or localhost). Open Lucidos over https://, an SSH tunnel to localhost, or tailscale serve: plain http://<host> cannot register notifications.';
  }
  if (!ctx.hasServiceWorker || !ctx.hasPushManager) {
    return 'Push notifications are not supported in this browser';
  }
  if (!ctx.hasNotification) {
    return 'Notifications are not supported in this browser';
  }
  return null;
}

export function pushUnsupportedReasonHere(httpsOrigin: string | null = null): string | null {
  return pushUnsupportedReason({
    httpsOrigin,
    secureContext: window.isSecureContext,
    hasServiceWorker: 'serviceWorker' in navigator,
    hasPushManager: 'PushManager' in window,
    hasNotification: 'Notification' in window,
    devServerBundle: isDevServerBundle(),
  });
}

async function gatewayHttpsOrigin(): Promise<string | null> {
  if (WORKSPACE_ID === null) return null;
  try {
    return (await getGatewayTls()).https_origin;
  } catch {
    return null;
  }
}

export async function initPushSubscription(): Promise<boolean> {
  if (isTauri()) {
    showToast('Notifications enabled', 'success');
    return true;
  }

  if (isIOS() && !isStandalone()) {
    showToast('On iOS, add Lucidos to your home screen first to enable push notifications', 'error');
    return false;
  }

  const unsupported = pushUnsupportedReasonHere(
    window.isSecureContext ? null : await gatewayHttpsOrigin(),
  );
  if (unsupported) {
    showToast(unsupported, 'error');
    return false;
  }

  try {
    await navigator.serviceWorker.register(workspaceServiceWorkerUrl(), { scope: SCOPE_PATH });

    const permission = await Notification.requestPermission();
    if (permission !== 'granted') {
      showToast('Notification permission was denied', 'error');
      return false;
    }

    const registration = await navigator.serviceWorker.ready;
    await subscribePush(await ensurePushSubscription(registration));
    showToast('Push notifications enabled', 'success');
    return true;
  } catch (err) {
    showToast(`Failed to enable push notifications: ${errorDetail(err)}`, 'error');
    return false;
  }
}

export function otherPushEnabledMobileDevices(
  all: DeviceInfo[],
  currentDeviceId: string,
): DeviceInfo[] {
  return all.filter(
    (d) => d.id !== currentDeviceId && d.push_enabled && isMobileDeviceUserAgent(d.user_agent),
  );
}

function deviceLabel(device: DeviceInfo): string {
  return deviceName(device.id, device, pairedRows(pairedDevices.value)?.find((p) => p.id === device.id));
}

async function offerToSilenceOtherMobileDevices(currentDeviceId: string): Promise<void> {
  if (!thisDeviceIsMobile()) return;
  const loaded = devices.value;
  if (loaded.status !== 'loaded') return;
  if (loaded.data.find((d) => d.id === currentDeviceId)?.push_enabled !== true) return;
  const others = otherPushEnabledMobileDevices(loaded.data, currentDeviceId);
  if (others.length === 0) return;

  const ok = await showConfirm(
    others.length === 1
      ? 'Your other phone still gets a push for everything too. Turn it off there, so only this device buzzes?'
      : `Your other ${others.length} mobile devices still get a push for everything too. Turn it off there, so only this device buzzes?`,
    'Turn off',
    {
      variant: 'default',
      title: 'Only notify this device?',
      cancelLabel: 'Keep them on',
      details: {
        groups: [{ header: 'Push will be turned off on', items: others.map(deviceLabel) }],
      },
    },
  );
  if (!ok) return;
  await disablePushForDevices(others.map((d) => d.id));
}

export async function setDevicePushEnabled(deviceId: string, enabled: boolean): Promise<void> {
  if (enabled && !(await initPushSubscription())) return;
  await toggleDevicePush(deviceId, enabled);
  if (enabled && deviceId === getDeviceId()) {
    await offerToSilenceOtherMobileDevices(deviceId);
  }
}
