import { API, json, mutatingFetch, throwIfNotOk } from '../../api/client';
import { showToast, showConfirm } from '../store';
import { devices, getDeviceId, toggleDevicePush, disablePushForDevices } from './devices';
import { isTauri, isIOS, isStandalone, isMobileDeviceUserAgent, thisDeviceIsMobile, describeDeviceUserAgent } from '../../utils/platform';
import { errorDetail } from '../../utils/errorDetail';
import { withBase, SCOPE_PATH } from '../../utils/basePath';
import { isDevServerBundle, DEV_SERVER_SW_REASON } from '../../utils/devServerBundle';
import type { DeviceInfo } from '../../api/types';

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
      scope_url: new URL(SCOPE_PATH, window.location.origin).toString(),
    }),
  });
  await throwIfNotOk(res);
}

/**
 * Does `sub` speak this engine's VAPID key? `null` means "can't tell", i.e. a
 * browser that doesn't expose `options.applicationServerKey`, which is
 * deliberately NOT treated as a mismatch: `subscribe()` itself is then the
 * authority (see `ensurePushSubscription`).
 */
function subscriptionUsesVapidKey(sub: PushSubscription, vapidKey: Uint8Array): boolean | null {
  const raw = sub.options?.applicationServerKey;
  if (!raw) return null;
  const bytes = new Uint8Array(raw);
  return bytes.length === vapidKey.length && bytes.every((b, i) => b === vapidKey[i]);
}

/** The Push API's spec-mandated rejection for "this registration already has a
 *  subscription under a DIFFERENT applicationServerKey". */
const STALE_KEY_ERROR = 'InvalidStateError';

/**
 * The browser-side subscription for this registration, reconciled against the
 * engine's CURRENT VAPID public key.
 *
 * The reconciliation is load-bearing, not defensive. VAPID keys are per
 * workspace (`vapid_keys` in that workspace's `preferences`), while behind the
 * gateway every workspace shares one origin and takes a `/<slug>/` service-worker
 * scope. So a workspace *recreated at the same slug* gets a fresh keypair while
 * the browser keeps the previous incarnation's subscription at that unchanged
 * scope. Without this, the two paths fail in opposite directions: enabling push
 * dies on the browser's "A subscription with a different applicationServerKey
 * already exists" (permanently: nothing in the UI could clear it), and the
 * background refresh silently re-POSTs a subscription this engine can never sign
 * a push for, so the user just gets no notifications.
 *
 * A subscription already on the current key is returned as-is; `subscribe()`
 * would only hand back that same object.
 */
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
    // Reaching here with a non-null `existing` means exactly one thing: it
    // held a subscription whose key we could not read, so the browser is the
    // first to know it is stale. Drop it and take the browser's own advice
    // ("unsubscribe then resubscribe"). Bounded to one retry, and gated on
    // `existing` (rather than a fresh read) so the retry fires only under
    // that precondition. The other documented source of InvalidStateError,
    // subscribing before the worker is active, cannot reach it: a caller
    // holding an inactive registration is holding a brand-new one, which has
    // no subscription to drop.
    if (!existing || (err as { name?: string })?.name !== STALE_KEY_ERROR) throw err;
    await existing.unsubscribe();
    return subscribe();
  }
}

/**
 * Silently re-subscribe push if already permitted.
 * Called on every page load to keep the endpoint fresh in the backend.
 *
 * Self-heals three divergent states that leave the engine pushing into the
 * void while the user sees silence. Two of them leave `devices.push_enabled =
 * true` with no `push_subscriptions` row: (1) a device flag written without a
 * completed browser handshake, which every enable path now refuses to do (see
 * `setDevicePushEnabled`) but which existing rows can still carry, since the
 * LLM `enable_push_notifications` tool used to flip the flag regardless;
 * (2) the browser loses the subscription (cleared site
 * data, SW unregistered). The third leaves a row that looks healthy but is
 * addressed with another workspace incarnation's VAPID key, and is repaired by
 * `ensurePushSubscription`. In all three `Notification.permission` stays
 * 'granted', so we can `subscribe()` silently without a permission prompt.
 */
export async function refreshPushSubscription(registration: ServiceWorkerRegistration): Promise<void> {
  try {
    if (isTauri()) return;
    if (!('PushManager' in window)) return;
    if (Notification.permission !== 'granted') return;

    // Re-send to the backend either way: an unchanged subscription still
    // refreshes device_id and replaces stale rows.
    await subscribePush(await ensurePushSubscription(registration));
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
  // A dev-server bundle registered none, so there is nothing wedged to recover
  // and registering one here would be the very thing the gate exists to prevent.
  if (isDevServerBundle()) return;
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
  await navigator.serviceWorker.register(withBase('/sw.js'), { scope: SCOPE_PATH, updateViaCache: 'none' });
  // pushManager.subscribe requires an active worker; calling pre-activation
  // rejects with InvalidStateError. `ready` resolves to the registration that
  // owns the active worker — passing the post-register registration directly
  // can hand refreshPushSubscription one still in 'installing'.
  const active = await navigator.serviceWorker.ready;
  await refreshPushSubscription(active);
}

/**
 * Why push can NEVER work in this page context, or `null` when it can.
 * Pure (all inputs injected) — exported for tests; production callers use
 * `pushUnsupportedReasonHere()`.
 *
 * The secure-context check comes first and is the load-bearing one for a
 * packaged install reached over plain `http://<host>:<port>/`: service workers
 * and `Notification.requestPermission` exist only in secure contexts (https or
 * localhost), and Chrome hides `navigator.serviceWorker` entirely there — so
 * without this check the user got a misleading "not supported in this browser"
 * for what is really an origin problem they can fix (open over https://, an SSH
 * tunnel to localhost, or `tailscale serve`).
 */
export function pushUnsupportedReason(ctx: {
  secureContext: boolean;
  hasServiceWorker: boolean;
  hasPushManager: boolean;
  hasNotification: boolean;
  devServerBundle: boolean;
}): string | null {
  // First, and above the secure-origin check, because a preview IS served over
  // https on localhost: every other message here would be advice the user has
  // already followed, and following it again would change nothing.
  if (ctx.devServerBundle) {
    return DEV_SERVER_SW_REASON;
  }
  if (!ctx.secureContext) {
    return 'Push needs a secure origin (https or localhost). Open Lucidos over https://, an SSH tunnel to localhost, or tailscale serve — plain http://<host> cannot register notifications.';
  }
  if (!ctx.hasServiceWorker || !ctx.hasPushManager) {
    return 'Push notifications are not supported in this browser';
  }
  if (!ctx.hasNotification) {
    return 'Notifications are not supported in this browser';
  }
  return null;
}

/** The production wiring of `pushUnsupportedReason` against the live page. */
export function pushUnsupportedReasonHere(): string | null {
  return pushUnsupportedReason({
    secureContext: window.isSecureContext,
    hasServiceWorker: 'serviceWorker' in navigator,
    hasPushManager: 'PushManager' in window,
    hasNotification: 'Notification' in window,
    devServerBundle: isDevServerBundle(),
  });
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

  const unsupported = pushUnsupportedReasonHere();
  if (unsupported) {
    showToast(unsupported, 'error');
    return false;
  }

  try {
    await navigator.serviceWorker.register(withBase('/sw.js'), { scope: SCOPE_PATH });

    const permission = await Notification.requestPermission();
    if (permission !== 'granted') {
      showToast('Notification permission was denied', 'error');
      return false;
    }

    // Wait for the ACTIVE worker rather than using the registration
    // `register()` hands back, for the same reason `recoverServiceWorker`
    // does: `subscribe()` requires an active worker and rejects with
    // InvalidStateError against one still in 'installing', which is what a
    // first-ever registration for this scope returns.
    const registration = await navigator.serviceWorker.ready;
    await subscribePush(await ensurePushSubscription(registration));
    showToast('Push notifications enabled', 'success');
    return true;
  } catch (err) {
    showToast(`Failed to enable push notifications: ${errorDetail(err)}`, 'error');
    return false;
  }
}

/** The OTHER devices that are phones/tablets and still have push on. Pure (the
 *  list is passed in) so the selection rule is testable without the store. */
export function otherPushEnabledMobileDevices(
  all: DeviceInfo[],
  currentDeviceId: string,
): DeviceInfo[] {
  return all.filter(
    (d) => d.id !== currentDeviceId && d.push_enabled && isMobileDeviceUserAgent(d.user_agent),
  );
}

/** How a device is named in prose the user reads: what they called it, else what
 *  its user-agent says it is. */
function deviceLabel(device: DeviceInfo): string {
  return device.name || describeDeviceUserAgent(device.user_agent);
}

/**
 * Having just turned push on for the phone in the user's hand, offer to turn it
 * off on their OTHER phones. A notification fans out to every push-enabled
 * device, so a spare handset, or a reinstalled PWA that minted a fresh device
 * id, keeps buzzing in a drawer with nobody to read it. Cancelling leaves every
 * device exactly as it was.
 *
 * Offered only FROM a mobile device, and only ABOUT mobile ones: a laptop and a
 * phone are complementary surfaces, and enabling push on one says nothing about
 * whether the other should stay quiet.
 *
 * The list is read from whatever `loadDevices` last fetched, which
 * `toggleDevicePush` has just refreshed, so the offer reflects the state after
 * this device was switched on.
 */
async function offerToSilenceOtherMobileDevices(currentDeviceId: string): Promise<void> {
  if (!thisDeviceIsMobile()) return;
  const loaded = devices.value;
  if (loaded.status !== 'loaded') return;
  // Only offer once THIS device is confirmed on. `toggleDevicePush` swallows a
  // failed write into a toast and returns normally, so without this check a
  // failed enable would still ask to silence the other phones, and confirming
  // would leave the user with no device getting a push at all.
  if (loaded.data.find((d) => d.id === currentDeviceId)?.push_enabled !== true) return;
  const others = otherPushEnabledMobileDevices(loaded.data, currentDeviceId);
  if (others.length === 0) return;

  const ok = await showConfirm(
    others.length === 1
      ? 'Your other phone still gets a push for everything too. Turn it off there, so only this device buzzes?'
      : `Your other ${others.length} mobile devices still get a push for everything too. Turn it off there, so only this device buzzes?`,
    'Turn off',
    {
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

/**
 * The one way push is turned on or off anywhere in the client: the row in
 * Settings → Devices, the row in Appearance & Behavior → Notifications, and the
 * `PushNotificationRequested` prompt the LLM `enable_push_notifications` tool
 * raises (`thread-sync.ts`). One entry point so the three can never disagree
 * about what flipping it does.
 *
 * Enabling needs this browser's own subscription handshake first, because a
 * page can only ever create `pushManager.subscribe()` for ITSELF; the device
 * flag alone would leave the engine pushing to an endpoint that does not exist.
 * A refused permission (or an unsupported context) leaves the flag untouched,
 * so the toggle springs back rather than claiming an "on" the OS will not
 * honour. Disabling is just the flag, and works for any device from anywhere.
 */
export async function setDevicePushEnabled(deviceId: string, enabled: boolean): Promise<void> {
  if (enabled && !(await initPushSubscription())) return;
  await toggleDevicePush(deviceId, enabled);
  if (enabled && deviceId === getDeviceId()) {
    await offerToSilenceOtherMobileDevices(deviceId);
  }
}
