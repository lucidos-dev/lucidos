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

// A Vitest run IS a Vite dev-server bundle, so the live `isDevServerBundle()`
// reports true here and every flow below would short-circuit on the frontend
// preview's no-service-worker gate. These tests exercise the PRODUCTION path, so
// the predicate is pinned false; the gate itself is covered by the pure
// `pushUnsupportedReason` cases at the bottom of this file and by
// `utils/devServerBundle.test.ts`. `importOriginal` keeps the real message
// string, which one of those cases asserts on.
vi.mock('../../utils/devServerBundle', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/devServerBundle')>()),
  isDevServerBundle: () => false,
}));

const originalFetch = globalThis.fetch;

/** Generate a real 65-byte uncompressed VAPID point (0x04 || X(32) || Y(32))
 *  so `pushManager.subscribe`'s applicationServerKey validation accepts it.
 *  `seed` varies the point so two workspaces can be given distinct keys. */
function makeVapidKey(seed = 0): string {
  const bytes = new Uint8Array(65);
  bytes[0] = 0x04;
  for (let i = 1; i < bytes.length; i++) bytes[i] = (i + seed) % 256;
  let bin = '';
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/=/g, '').replace(/\+/g, '-').replace(/\//g, '_');
}

/** The raw bytes a browser reports in `subscription.options.applicationServerKey`
 *  for a subscription created against `key`. */
function vapidKeyBytes(key: string): ArrayBuffer {
  const bin = atob(key.replace(/-/g, '+').replace(/_/g, '/'));
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out.buffer;
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
        getSubscription: async () => null,
        subscribe: async () => ({
          toJSON: () => ({ endpoint: 'https://push/x', keys: { p256dh: 'p', auth: 'a' } }),
        }),
      },
    };
    Object.defineProperty(globalThis.navigator, 'serviceWorker', {
      value: { register: async () => fakeRegistration, ready: Promise.resolve(fakeRegistration) },
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

/**
 * Regression: a workspace recreated at the same gateway slug mints a fresh
 * VAPID keypair, but the browser keeps its subscription at the unchanged
 * `/<slug>/` service-worker scope. Enabling push then died on the browser's
 * "A subscription with a different applicationServerKey (or gcm_sender_id)
 * already exists" with no way out of the UI, while the background refresh
 * silently re-POSTed a subscription the engine could never sign a push for.
 */
describe('stale applicationServerKey reconciliation', () => {
  function sameBytes(a: ArrayBuffer, b: ArrayBuffer): boolean {
    const x = new Uint8Array(a);
    const y = new Uint8Array(b);
    return x.length === y.length && x.every((v, i) => v === y[i]);
  }

  /** A PushManager fake carrying the browser's real contract: `subscribe()`
   *  rejects with `InvalidStateError` while a subscription created under a
   *  different applicationServerKey is still present. `existing.key === null`
   *  models a browser that hides `options.applicationServerKey`; pair it with
   *  `hiddenKeyMismatch` to make that hidden key a stale one. */
  function makeRegistration(existing?: {
    key: string | null;
    endpoint: string;
    hiddenKeyMismatch?: boolean;
  }) {
    const calls: string[] = [];
    let fresh = 0;
    const makeSub = (keyBytes: ArrayBuffer | null, endpoint: string) => ({
      endpoint,
      options: { applicationServerKey: keyBytes },
      toJSON: () => ({ endpoint, keys: { p256dh: 'p', auth: 'a' } }),
      unsubscribe: async () => {
        calls.push(`unsubscribe:${endpoint}`);
        current = null;
        return true;
      },
    });
    let current: ReturnType<typeof makeSub> | null = existing
      ? makeSub(existing.key === null ? null : vapidKeyBytes(existing.key), existing.endpoint)
      : null;

    const registration = {
      pushManager: {
        getSubscription: async () => current,
        subscribe: async (opts: { applicationServerKey: ArrayBuffer }) => {
          calls.push('subscribe');
          if (current) {
            const reported = current.options.applicationServerKey;
            const matches = reported
              ? sameBytes(reported, opts.applicationServerKey)
              : !existing?.hiddenKeyMismatch;
            if (!matches) {
              const err = new Error(
                'Registration failed - A subscription with a different applicationServerKey (or gcm_sender_id) already exists; to change the applicationServerKey, unsubscribe then resubscribe.',
              );
              err.name = 'InvalidStateError';
              throw err;
            }
            return current;
          }
          fresh += 1;
          current = makeSub(opts.applicationServerKey, `https://fcm.googleapis.com/fcm/send/fresh-${fresh}`);
          return current;
        },
      },
    };
    return { registration, calls, subscription: () => current };
  }

  /** Serve `vapidKey` from the engine and accept every POST; returns the log. */
  function installFetch(vapidKey: string) {
    const seen: Array<{ url: string; method: string; body?: string }> = [];
    globalThis.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input.toString();
      seen.push({
        url,
        method: init?.method ?? 'GET',
        body: typeof init?.body === 'string' ? init.body : undefined,
      });
      if (url.endsWith('/api/v1/push/vapid-key')) {
        return new Response(JSON.stringify({ public_key: vapidKey }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      }
      return new Response('{}', { status: 200, headers: { 'content-type': 'application/json' } });
    }) as typeof fetch;
    return seen;
  }

  const subscribedEndpoint = (seen: Array<{ url: string; method: string; body?: string }>) => {
    const post = seen.find((s) => s.url.endsWith('/api/v1/push/subscribe') && s.method === 'POST');
    return post ? (JSON.parse(post.body!) as { endpoint: string }).endpoint : null;
  };

  const OLD_KEY = makeVapidKey(1);
  const CURRENT_KEY = makeVapidKey(2);
  const STALE_ENDPOINT = 'https://fcm.googleapis.com/fcm/send/previous-workspace';

  beforeEach(() => {
    localStorage.setItem('lucidos-device-id', 'dev-test-vapid');
    Object.defineProperty(window, 'location', {
      value: { origin: 'https://lucidos.test' },
      configurable: true,
    });
    installPermissionStubs();
  });
  afterEach(() => {
    globalThis.fetch = originalFetch;
    localStorage.removeItem('lucidos-device-id');
    vi.resetModules();
  });

  it('initPushSubscription replaces a subscription bound to a previous VAPID key instead of failing', async () => {
    const seen = installFetch(CURRENT_KEY);
    const { registration, calls } = makeRegistration({ key: OLD_KEY, endpoint: STALE_ENDPOINT });
    Object.defineProperty(globalThis.navigator, 'serviceWorker', {
      value: { register: async () => registration, ready: Promise.resolve(registration) },
      configurable: true,
    });

    const { initPushSubscription } = await import('./push');
    expect(
      await initPushSubscription(),
      'enabling push must recover from a stale applicationServerKey, not report failure',
    ).toBe(true);

    expect(calls).toEqual([`unsubscribe:${STALE_ENDPOINT}`, 'subscribe']);
    expect(subscribedEndpoint(seen)).toBe('https://fcm.googleapis.com/fcm/send/fresh-1');
  });

  it('refreshPushSubscription replaces the stale subscription rather than re-POSTing it', async () => {
    const seen = installFetch(CURRENT_KEY);
    const { registration, calls } = makeRegistration({ key: OLD_KEY, endpoint: STALE_ENDPOINT });

    const { refreshPushSubscription } = await import('./push');
    await refreshPushSubscription(registration as unknown as ServiceWorkerRegistration);

    expect(calls).toEqual([`unsubscribe:${STALE_ENDPOINT}`, 'subscribe']);
    expect(
      subscribedEndpoint(seen),
      'the engine must never be handed an endpoint it cannot sign a push for',
    ).toBe('https://fcm.googleapis.com/fcm/send/fresh-1');
  });

  it('a subscription already on the current key is reused, not churned', async () => {
    const seen = installFetch(CURRENT_KEY);
    const live = 'https://fcm.googleapis.com/fcm/send/still-good';
    const { registration, calls } = makeRegistration({ key: CURRENT_KEY, endpoint: live });

    const { refreshPushSubscription } = await import('./push');
    await refreshPushSubscription(registration as unknown as ServiceWorkerRegistration);

    expect(calls, 'a matching subscription needs neither unsubscribe nor subscribe').toEqual([]);
    expect(subscribedEndpoint(seen)).toBe(live);
  });

  it('recovers when the browser hides applicationServerKey and only subscribe() reveals the mismatch', async () => {
    const seen = installFetch(CURRENT_KEY);
    const { registration, calls } = makeRegistration({
      key: null,
      endpoint: STALE_ENDPOINT,
      hiddenKeyMismatch: true,
    });

    const { refreshPushSubscription } = await import('./push');
    await refreshPushSubscription(registration as unknown as ServiceWorkerRegistration);

    expect(calls).toEqual(['subscribe', `unsubscribe:${STALE_ENDPOINT}`, 'subscribe']);
    expect(subscribedEndpoint(seen)).toBe('https://fcm.googleapis.com/fcm/send/fresh-1');
  });

  it('an InvalidStateError with nothing to unsubscribe is surfaced, not retried', async () => {
    installFetch(CURRENT_KEY);
    const calls: string[] = [];
    const registration = {
      pushManager: {
        getSubscription: async () => null,
        subscribe: async () => {
          calls.push('subscribe');
          // What subscribing before the worker is active looks like.
          const err = new Error('Subscription failed - no active Service Worker');
          err.name = 'InvalidStateError';
          throw err;
        },
      },
    };
    Object.defineProperty(globalThis.navigator, 'serviceWorker', {
      value: { register: async () => registration, ready: Promise.resolve(registration) },
      configurable: true,
    });

    const { initPushSubscription } = await import('./push');
    expect(await initPushSubscription()).toBe(false);
    expect(calls, 'no subscription to drop means no retry loop').toEqual(['subscribe']);
  });

  it('subscribes against the active worker, not the registration register() returns', async () => {
    const seen = installFetch(CURRENT_KEY);
    // A first-ever registration for this scope comes back still 'installing',
    // and subscribe() rejects against it. `ready` resolves to the one that
    // owns the active worker.
    const installing = {
      pushManager: {
        getSubscription: async () => null,
        subscribe: async () => {
          const err = new Error('Subscription failed - no active Service Worker');
          err.name = 'InvalidStateError';
          throw err;
        },
      },
    };
    const { registration: active } = makeRegistration();
    Object.defineProperty(globalThis.navigator, 'serviceWorker', {
      value: { register: async () => installing, ready: Promise.resolve(active) },
      configurable: true,
    });

    const { initPushSubscription } = await import('./push');
    expect(
      await initPushSubscription(),
      'enabling push must wait for the active worker rather than subscribing pre-activation',
    ).toBe(true);
    expect(subscribedEndpoint(seen)).toBe('https://fcm.googleapis.com/fcm/send/fresh-1');
  });
});

describe('pushUnsupportedReason', () => {
  const ALL_OK = {
    secureContext: true,
    hasServiceWorker: true,
    hasPushManager: true,
    hasNotification: true,
    devServerBundle: false,
  };

  it('returns null when the context fully supports push', async () => {
    const { pushUnsupportedReason } = await import('./push');
    expect(pushUnsupportedReason(ALL_OK)).toBeNull();
  });

  it('the frontend preview wins over every other reason, because it is https already', async () => {
    // A preview is served over https on localhost, so `secureContext` is true
    // and the origin advice would be advice the user has already followed. It
    // registers no service worker by design (utils/devServerBundle.ts), so the
    // "not supported in this browser" message would blame the wrong thing too.
    const { pushUnsupportedReason } = await import('./push');
    const reason = pushUnsupportedReason({
      ...ALL_OK,
      devServerBundle: true,
      hasServiceWorker: false,
      secureContext: false,
    });
    expect(reason).toMatch(/preview/i);
    expect(reason).not.toMatch(/secure origin/i);
    expect(reason).not.toMatch(/not supported in this browser/i);
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
      devServerBundle: false,
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
