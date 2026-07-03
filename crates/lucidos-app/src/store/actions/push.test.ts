/**
 * Regression: `getVapidKey` used to call `fetch()` directly, bypassing the
 * shared `json()` helper. That dropped the `x-lucidos-device-id` header and
 * surfaced engine errors as a generic `TypeError("Failed to fetch")` instead
 * of the typed `ApiError` other helpers raise.
 *
 * These tests bind that contract to the network surface: every push action
 * that the SW route exposes goes through `json()` / `mutatingFetch`, both of
 * which set `x-lucidos-device-id`.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const originalFetch = globalThis.fetch;

/** Generate a real 65-byte uncompressed VAPID point (0x04 || X(32) || Y(32))
 *  so `pushManager.subscribe`'s applicationServerKey validation accepts it. */
function makeVapidKey(): string {
  const bytes = new Uint8Array(65);
  bytes[0] = 0x04;
  for (let i = 1; i < bytes.length; i++) bytes[i] = i;
  let bin = '';
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/=/g, '').replace(/\+/g, '-').replace(/\//g, '_');
}

/** Install a `granted`-permission Notification + PushManager stub on globalThis,
 *  matching what the production flow probes for before subscribing. The fake
 *  origin is https://lucidos.test (see beforeEach), so declare the secure
 *  context a real browser would grant it — initPushSubscription's
 *  pushUnsupportedReasonHere() gate checks window.isSecureContext first. */
function installPermissionStubs() {
  (globalThis as unknown as { Notification: typeof Notification }).Notification = Object.assign(
    function () { /* stub */ } as unknown as typeof Notification,
    {
      permission: 'granted',
      requestPermission: async () => 'granted',
    },
  ) as typeof Notification;
  (globalThis as unknown as { PushManager: object }).PushManager = function () { /* stub */ };
  Object.defineProperty(window, 'isSecureContext', { value: true, configurable: true });
}

describe('push.ts uses api/client helpers (not raw fetch)', () => {
  beforeEach(() => {
    localStorage.setItem('lucidos-device-id', 'dev-test-vapid');
    Object.defineProperty(window, 'location', {
      value: { origin: 'https://lucidos.test' },
      configurable: true,
    });
  });
  afterEach(() => {
    globalThis.fetch = originalFetch;
    localStorage.removeItem('lucidos-device-id');
    vi.resetModules();
  });

  it('initPushSubscription routes the VAPID-key request through json() (sends x-lucidos-device-id)', async () => {
    const seen: Array<{ url: string; headers: Record<string, string> }> = [];
    const vapidKey = makeVapidKey();
    globalThis.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input.toString();
      const headers = (init?.headers as Record<string, string>) ?? {};
      seen.push({ url, headers });
      if (url.endsWith('/api/v1/push/vapid-key')) {
        return new Response(JSON.stringify({ public_key: vapidKey }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      }
      // Push subscribe POST — accept and return empty body
      return new Response('{}', { status: 200, headers: { 'content-type': 'application/json' } });
    }) as typeof fetch;

    const fakeRegistration = {
      pushManager: {
        subscribe: async () => ({
          toJSON: () => ({ endpoint: 'https://push/x', keys: { p256dh: 'p', auth: 'a' } }),
        }),
      },
    };
    Object.defineProperty(globalThis.navigator, 'serviceWorker', {
      value: { register: async () => fakeRegistration },
      configurable: true,
    });
    installPermissionStubs();

    const { initPushSubscription } = await import('./push');
    const ok = await initPushSubscription();
    expect(ok).toBe(true);

    const vapid = seen.find((s) => s.url.endsWith('/api/v1/push/vapid-key'));
    expect(vapid, 'a request to /api/v1/push/vapid-key must have been made').toBeDefined();
    expect(
      vapid!.headers['x-lucidos-device-id'],
      'VAPID call must go through json() so the device-id header is attached',
    ).toBe('dev-test-vapid');
  });

  /**
   * Regression guard for the divergence where `devices.push_enabled = true`
   * yet `push_subscriptions` has no row for the device, so the engine pushes
   * to no Chrome endpoint and the user sees nothing. Two known causes: the
   * LLM `enable_push_notifications` tool flips the device flag before the
   * browser handshake completes, or the browser silently loses its
   * subscription (cleared site data, SW unregistered). Permission stays
   * 'granted', so the recovery path is silent.
   */
  it('refreshPushSubscription self-heals when permission granted but subscription missing', async () => {
    const seen: Array<{ url: string; method: string; body?: string }> = [];
    const vapidKey = makeVapidKey();

    globalThis.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input.toString();
      const method = init?.method ?? 'GET';
      const body = typeof init?.body === 'string' ? init.body : undefined;
      seen.push({ url, method, body });
      if (url.endsWith('/api/v1/push/vapid-key')) {
        return new Response(JSON.stringify({ public_key: vapidKey }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      }
      return new Response('{}', { status: 200, headers: { 'content-type': 'application/json' } });
    }) as typeof fetch;

    const subscribeCalls: Array<unknown> = [];
    const fakeRegistration = {
      pushManager: {
        getSubscription: async () => null,
        subscribe: async (opts: unknown) => {
          subscribeCalls.push(opts);
          return {
            toJSON: () => ({
              endpoint: 'https://fcm.googleapis.com/fcm/send/self-heal',
              keys: { p256dh: 'p-self', auth: 'a-self' },
            }),
          };
        },
      },
    };

    installPermissionStubs();

    const { refreshPushSubscription } = await import('./push');
    await refreshPushSubscription(fakeRegistration as unknown as ServiceWorkerRegistration);

    expect(
      subscribeCalls.length,
      'pushManager.subscribe must be called when permission granted but no existing subscription',
    ).toBe(1);

    const subscribePost = seen.find(
      (s) => s.url.endsWith('/api/v1/push/subscribe') && s.method === 'POST',
    );
    expect(
      subscribePost,
      'newly-created subscription must be POSTed to /api/v1/push/subscribe',
    ).toBeDefined();
    expect(subscribePost!.body).toContain('self-heal');
    const subscribePayload = JSON.parse(subscribePost!.body!) as Record<string, string>;
    expect(subscribePayload.scope_url).toBe('https://lucidos.test/');
  });
});

describe('pushUnsupportedReason', () => {
  const ALL_OK = {
    secureContext: true,
    hasServiceWorker: true,
    hasPushManager: true,
    hasNotification: true,
  };

  it('returns null when the context fully supports push', async () => {
    const { pushUnsupportedReason } = await import('./push');
    expect(pushUnsupportedReason(ALL_OK)).toBeNull();
  });

  it('insecure origin wins over missing APIs and names the fix, not the browser', async () => {
    // Chrome hides navigator.serviceWorker entirely on insecure origins, so
    // this exact combination is what a packaged install over http://<host>
    // reports — the message must blame the origin and point at https/localhost.
    const { pushUnsupportedReason } = await import('./push');
    const reason = pushUnsupportedReason({
      secureContext: false,
      hasServiceWorker: false,
      hasPushManager: false,
      hasNotification: true,
    });
    expect(reason).toMatch(/secure origin/i);
    expect(reason).toMatch(/https/);
    expect(reason).not.toMatch(/not supported in this browser/i);
  });

  it('missing service worker / PushManager on a secure origin → browser support message', async () => {
    const { pushUnsupportedReason } = await import('./push');
    expect(pushUnsupportedReason({ ...ALL_OK, hasServiceWorker: false })).toMatch(/not supported in this browser/i);
    expect(pushUnsupportedReason({ ...ALL_OK, hasPushManager: false })).toMatch(/not supported in this browser/i);
  });

  it('missing Notification API → notifications support message', async () => {
    const { pushUnsupportedReason } = await import('./push');
    expect(pushUnsupportedReason({ ...ALL_OK, hasNotification: false })).toMatch(/notifications are not supported/i);
  });
});
