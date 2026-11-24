import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../utils/devServerBundle', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/devServerBundle')>()),
  isDevServerBundle: () => false,
}));

const originalFetch = globalThis.fetch;

function makeVapidKey(seed = 0): string {
  const bytes = new Uint8Array(65);
  bytes[0] = 0x04;
  for (let i = 1; i < bytes.length; i++) bytes[i] = (i + seed) % 256;
  let bin = '';
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/=/g, '').replace(/\+/g, '-').replace(/\//g, '_');
}

function vapidKeyBytes(key: string): ArrayBuffer {
  const bin = atob(key.replace(/-/g, '+').replace(/_/g, '/'));
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out.buffer;
}

function installPermissionStubs() {
  (globalThis as unknown as { Notification: typeof Notification }).Notification = Object.assign(
    function () {  } as unknown as typeof Notification,
    {
      permission: 'granted',
      requestPermission: async () => 'granted',
    },
  ) as typeof Notification;
  (globalThis as unknown as { PushManager: object }).PushManager = function () {  };
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
      return new Response('{"success":true}', { status: 200, headers: { 'content-type': 'application/json' } });
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

  it('initPushSubscription reports a subscribe the engine refused', async () => {
    const vapidKey = makeVapidKey();
    globalThis.fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = typeof input === 'string' ? input : input.toString();
      if (url.endsWith('/api/v1/push/vapid-key')) {
        return new Response(JSON.stringify({ public_key: vapidKey }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      }
      return new Response(
        JSON.stringify({ success: false, error: 'this caller is device dev-a and cannot act for device dev-b' }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      );
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
    expect(await initPushSubscription()).toBe(false);
  });

  it('a relayed page registers and subscribes under its outside prefix', async () => {
    const stampedBase = { getAttribute: (name: string) => (name === 'href' ? '/dev@cloud/' : null) };
    const querySelector = vi
      .spyOn(document, 'querySelector')
      .mockImplementation(((selector: string) =>
        selector === 'base' ? stampedBase : null) as unknown as typeof document.querySelector);
    try {
      const seen: Array<{ url: string; body?: string }> = [];
      globalThis.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === 'string' ? input : input.toString();
        seen.push({ url, body: typeof init?.body === 'string' ? init.body : undefined });
        if (url.endsWith('/api/v1/push/vapid-key')) {
          return new Response(JSON.stringify({ public_key: makeVapidKey() }), {
            status: 200,
            headers: { 'content-type': 'application/json' },
          });
        }
        return new Response('{"success":true}', { status: 200, headers: { 'content-type': 'application/json' } });
      }) as typeof fetch;

      const fakeRegistration = {
        pushManager: {
          getSubscription: async () => null,
          subscribe: async () => ({
            toJSON: () => ({ endpoint: 'https://push/relayed', keys: { p256dh: 'p', auth: 'a' } }),
          }),
        },
      };
      const registrations: Array<{ script: string; scope?: string }> = [];
      Object.defineProperty(globalThis.navigator, 'serviceWorker', {
        value: {
          register: async (script: string, opts?: RegistrationOptions) => {
            registrations.push({ script, scope: opts?.scope });
            return fakeRegistration;
          },
          ready: Promise.resolve(fakeRegistration),
          getRegistration: async () => undefined,
        },
        configurable: true,
      });
      installPermissionStubs();

      const { initPushSubscription, recoverServiceWorker } = await import('./push');
      expect(await initPushSubscription()).toBe(true);

      expect(registrations).toContainEqual({ script: '/~/sw.js?workspace=dev%40cloud', scope: '/dev@cloud/' });
      await recoverServiceWorker();
      expect(registrations).toHaveLength(2);
      expect(registrations[1]).toEqual(registrations[0]);
      const subscribe = seen.find((s) => s.url === '/dev@cloud/api/v1/push/subscribe');
      expect(subscribe, 'the subscribe POST must go under the outside prefix').toBeDefined();
      const payload = JSON.parse(subscribe!.body!) as Record<string, string>;
      expect(payload.scope_url).toBe('https://lucidos.test/dev@cloud/');
    } finally {
      querySelector.mockRestore();
    }
  });

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
      return new Response('{"success":true}', { status: 200, headers: { 'content-type': 'application/json' } });
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

describe('stale applicationServerKey reconciliation', () => {
  function sameBytes(a: ArrayBuffer, b: ArrayBuffer): boolean {
    const x = new Uint8Array(a);
    const y = new Uint8Array(b);
    return x.length === y.length && x.every((v, i) => v === y[i]);
  }

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
      return new Response('{"success":true}', { status: 200, headers: { 'content-type': 'application/json' } });
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

  it("names the gateway's own HTTPS origin when it serves one", async () => {
    const { pushUnsupportedReason } = await import('./push');
    const reason = pushUnsupportedReason({
      ...ALL_OK,
      secureContext: false,
      hasServiceWorker: false,
      httpsOrigin: 'https://mymac.tailnet-name.ts.net:5252',
    });
    expect(reason).toContain('Open Lucidos at https://mymac.tailnet-name.ts.net:5252');
    expect(pushUnsupportedReason({ ...ALL_OK, httpsOrigin: 'https://mymac.tailnet-name.ts.net:5252' })).toBeNull();
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
