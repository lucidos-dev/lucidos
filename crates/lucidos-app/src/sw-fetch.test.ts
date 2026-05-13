import { describe, it, expect, vi, beforeEach } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const swSource = readFileSync(resolve(__dirname, '../public/sw.js'), 'utf-8');

// Runs sw.js inside a sandbox where `self`, `fetch`, `clients`, and `caches`
// are mocks. Returns the registered fetch handler so tests can drive it.
// Top-level handlers (push, notificationclick) only register their listeners —
// they don't fire at load time, so the mocks just need to satisfy the
// addEventListener calls.
function loadSw() {
  const handlers: Record<string, (event: any) => void> = {};
  const mockFetch = vi.fn();
  const cacheStore = new Map<string, Response>();
  const mockCache = {
    match: vi.fn((req: { url: string } | string) =>
      Promise.resolve(cacheStore.get(typeof req === 'string' ? req : req.url)),
    ),
    put: vi.fn((req: { url: string } | string, res: Response) => {
      cacheStore.set(typeof req === 'string' ? req : req.url, res);
      return Promise.resolve();
    }),
  };
  const mockCaches = {
    open: vi.fn(() => Promise.resolve(mockCache)),
  };
  const mockSelf = {
    addEventListener: (type: string, handler: (event: any) => void) => {
      handlers[type] = handler;
    },
    location: { origin: 'https://example.com' },
  };
  // eslint-disable-next-line @typescript-eslint/no-implied-eval
  new Function('self', 'fetch', 'clients', 'caches', swSource)(mockSelf, mockFetch, {}, mockCaches);
  return { handlers, mockFetch, mockCache, mockCaches, cacheStore };
}

function makeEvent(url: string, method: string = 'GET') {
  return {
    request: { url, method },
    respondWith: vi.fn(),
  };
}

describe('Service Worker fetch handler', () => {
  let handlers: Record<string, (event: any) => void>;
  let mockFetch: ReturnType<typeof vi.fn>;
  let mockCache: ReturnType<typeof loadSw>['mockCache'];
  let cacheStore: Map<string, Response>;

  beforeEach(() => {
    const sw = loadSw();
    handlers = sw.handlers;
    mockFetch = sw.mockFetch;
    mockCache = sw.mockCache;
    cacheStore = sw.cacheStore;
  });

  it('GET to /api/foo: calls respondWith (needed for iOS empty-response fix)', () => {
    mockFetch.mockResolvedValue(new Response('ok'));
    const event = makeEvent('https://example.com/api/threads/abc/events');
    handlers.fetch(event);
    expect(event.respondWith).toHaveBeenCalledTimes(1);
  });

  it('POST to /api/foo: does NOT call respondWith (browser handles natively, avoids iOS body-clone bug)', () => {
    const event = makeEvent('https://example.com/api/chat', 'POST');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('PUT to /api/foo: does NOT call respondWith', () => {
    const event = makeEvent('https://example.com/api/preferences', 'PUT');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('DELETE to /api/foo: does NOT call respondWith', () => {
    const event = makeEvent('https://example.com/api/threads/abc', 'DELETE');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('GET to /api/events (SSE): does NOT call respondWith', () => {
    const event = makeEvent('https://example.com/api/events');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('GET to /api/events with query string: does NOT call respondWith', () => {
    const event = makeEvent('https://example.com/api/events?since=42');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('cross-origin GET: does NOT call respondWith', () => {
    const event = makeEvent('https://other.com/api/foo');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('non-API GET (static asset): does NOT call respondWith', () => {
    const event = makeEvent('https://example.com/index.html');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('GET retries once if first fetch throws (covers iOS SW restart race)', async () => {
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce(new Response('ok'));
    const event = makeEvent('https://example.com/api/threads/abc/events');
    handlers.fetch(event);
    expect(event.respondWith).toHaveBeenCalledTimes(1);
    const response = await event.respondWith.mock.calls[0][0];
    expect(mockFetch).toHaveBeenCalledTimes(2);
    expect(response).toBeInstanceOf(Response);
  });

  it('GET propagates error if both attempts fail', async () => {
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockRejectedValueOnce(new TypeError('Load failed'));
    const event = makeEvent('https://example.com/api/threads/abc/events');
    handlers.fetch(event);
    await expect(event.respondWith.mock.calls[0][0]).rejects.toThrow('Load failed');
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });

  // Content-addressed blob endpoints (immutable for the lifetime of the
  // hash) get persisted in the Cache API so iOS PWA — which evicts the HTTP
  // cache aggressively — doesn't re-fetch on every thread visit. The visible
  // symptom of the eviction is a brief black flash where the empty <img>
  // shows the dark page background through it before the bytes arrive.
  it('GET /api/v1/blobs/<hash>/preview: serves from Cache API on hit (no network)', async () => {
    const cached = new Response('cached-bytes');
    cacheStore.set('https://example.com/api/v1/blobs/abc/preview', cached);
    const event = makeEvent('https://example.com/api/v1/blobs/abc/preview');
    handlers.fetch(event);
    expect(event.respondWith).toHaveBeenCalledTimes(1);
    const response = await event.respondWith.mock.calls[0][0];
    expect(response).toBe(cached);
    expect(mockFetch).not.toHaveBeenCalled();
  });

  it('GET /api/v1/blobs/<hash>: serves from Cache API on hit (original blob URL too)', async () => {
    const cached = new Response('orig-bytes');
    cacheStore.set('https://example.com/api/v1/blobs/abc', cached);
    const event = makeEvent('https://example.com/api/v1/blobs/abc');
    handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(response).toBe(cached);
    expect(mockFetch).not.toHaveBeenCalled();
  });

  it('GET /api/v1/blobs/<hash>/preview: caches successful response on miss', async () => {
    mockFetch.mockResolvedValue(new Response('fresh', { status: 200 }));
    const event = makeEvent('https://example.com/api/v1/blobs/abc/preview');
    handlers.fetch(event);
    await event.respondWith.mock.calls[0][0];
    expect(mockFetch).toHaveBeenCalledTimes(1);
    expect(mockCache.put).toHaveBeenCalledTimes(1);
    const cachedReq = mockCache.put.mock.calls[0][0] as { url: string };
    expect(cachedReq.url).toBe('https://example.com/api/v1/blobs/abc/preview');
  });

  it('GET /api/v1/blobs/<hash>/preview: does NOT cache failed response (404, 5xx)', async () => {
    mockFetch.mockResolvedValue(new Response('not found', { status: 404 }));
    const event = makeEvent('https://example.com/api/v1/blobs/abc/preview');
    handlers.fetch(event);
    await event.respondWith.mock.calls[0][0];
    expect(mockCache.put).not.toHaveBeenCalled();
  });

  it('GET /api/threads/abc/events: still uses fetchWithRetry path (NOT Cache API)', async () => {
    mockFetch.mockResolvedValue(new Response('ok'));
    const event = makeEvent('https://example.com/api/threads/abc/events');
    handlers.fetch(event);
    await event.respondWith.mock.calls[0][0];
    expect(mockCache.put).not.toHaveBeenCalled();
    expect(mockCache.match).not.toHaveBeenCalled();
  });
});

function makeNotificationCloseEvent(notificationId: string | null, tag?: string) {
  const data = notificationId ? { notification_id: notificationId } : undefined;
  return {
    notification: {
      data,
      tag: tag ?? notificationId ?? 'lucidos-notification',
    },
    waitUntil: vi.fn((p: Promise<any>) => p),
  };
}

describe('Service Worker notificationclose handler', () => {
  let handlers: Record<string, (event: any) => void>;
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    const sw = loadSw();
    handlers = sw.handlers;
    mockFetch = sw.mockFetch;
    mockFetch.mockResolvedValue(new Response('ok'));
  });

  it('POSTs notification id to /api/notification-dismissed when user dismisses the OS notification', () => {
    const event = makeNotificationCloseEvent('notif-abc');
    handlers.notificationclose(event);

    expect(mockFetch).toHaveBeenCalledTimes(1);
    const [url, init] = mockFetch.mock.calls[0];
    expect(url).toBe('https://example.com/api/notification-dismissed');
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body)).toEqual({ notification_id: 'notif-abc' });
  });

  it('falls back to the notification tag when data.notification_id is missing (iOS)', () => {
    const event = makeNotificationCloseEvent(null, 'notif-xyz');
    handlers.notificationclose(event);

    expect(mockFetch).toHaveBeenCalledTimes(1);
    expect(JSON.parse(mockFetch.mock.calls[0][1].body)).toEqual({ notification_id: 'notif-xyz' });
  });

  it('does nothing when there is no notification id and the tag is the default', () => {
    const event = makeNotificationCloseEvent(null, 'lucidos-notification');
    handlers.notificationclose(event);

    expect(mockFetch).not.toHaveBeenCalled();
  });

  it('swallows fetch errors so the close handler does not throw', async () => {
    mockFetch.mockRejectedValueOnce(new TypeError('Load failed'));
    const event = makeNotificationCloseEvent('notif-abc');
    handlers.notificationclose(event);

    await expect(event.waitUntil.mock.calls[0][0]).resolves.toBeUndefined();
  });
});
