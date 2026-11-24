import { describe, it, expect, vi, beforeEach } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const swSource = readFileSync(resolve(__dirname, '../public/sw.js'), 'utf-8');

const SHELL_CACHE = 'lucidos-shell-__LUCIDOS_BUILD_ID__';

type FakeClient = {
  frameType: string;
  visibilityState: string;
  url?: string;
  postMessage?: (msg: unknown) => void;
  focus?: () => Promise<unknown>;
  navigate?: (url: string) => Promise<unknown>;
};

function loadSw(opts: { buildId?: string; scope?: string } = {}) {
  const source = opts.buildId
    ? swSource.replace(/__LUCIDOS_BUILD_ID__/g, opts.buildId)
    : swSource;
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
  const cacheNames = new Set<string>();
  const mockCaches = {
    open: vi.fn((name: string) => { cacheNames.add(name); return Promise.resolve(mockCache); }),
    keys: vi.fn(() => Promise.resolve([...cacheNames])),
    delete: vi.fn((name: string) => Promise.resolve(cacheNames.delete(name))),
  };
  const mockRegistration = {
    showNotification: vi.fn((_title: string, _opts: Record<string, unknown>) => Promise.resolve()),
    scope: opts.scope ?? 'https://example.com/',
  };
  const setAppBadge = vi.fn((_count?: number) => Promise.resolve());
  const clearAppBadge = vi.fn(() => Promise.resolve());
  const mockSelf = {
    addEventListener: (type: string, handler: (event: any) => void) => {
      handlers[type] = handler;
    },
    skipWaiting: vi.fn(),
    location: { origin: 'https://example.com' },
    registration: mockRegistration,
    navigator: { setAppBadge, clearAppBadge },
  };
  const matchAll = vi.fn(() => Promise.resolve([] as FakeClient[]));
  const openWindow = vi.fn(() => Promise.resolve(null));
  const mockClients = { matchAll, openWindow, claim: () => Promise.resolve() };
  // eslint-disable-next-line @typescript-eslint/no-implied-eval
  new Function('self', 'fetch', 'clients', 'caches', source)(mockSelf, mockFetch, mockClients, mockCaches);
  return {
    handlers,
    mockFetch,
    mockCache,
    mockCaches,
    cacheStore,
    cacheNames,
    mockRegistration,
    matchAll,
    openWindow,
    skipWaiting: mockSelf.skipWaiting,
    setAppBadge,
    clearAppBadge,
  };
}

function makeEvent(url: string, method: string = 'GET', mode?: string) {
  return {
    request: { url, method, ...(mode ? { mode } : {}) },
    respondWith: vi.fn(),
    waitUntil: vi.fn(),
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

  it('GET to /api/v1/foo: bypasses the worker', () => {
    mockFetch.mockResolvedValue(new Response('ok'));
    const event = makeEvent('https://example.com/api/v1/threads/abc/events');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('POST to /api/v1/foo: does NOT call respondWith (browser handles natively, avoids iOS body-clone bug)', () => {
    const event = makeEvent('https://example.com/api/v1/chat/stream', 'POST');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('PUT to /api/v1/foo: does NOT call respondWith', () => {
    const event = makeEvent('https://example.com/api/v1/preferences', 'PUT');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('DELETE to /api/v1/foo: does NOT call respondWith', () => {
    const event = makeEvent('https://example.com/api/v1/threads/abc', 'DELETE');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('GET to /api/v1/events (SSE): does NOT call respondWith', () => {
    const event = makeEvent('https://example.com/api/v1/events');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('GET to /api/v1/events with query string: does NOT call respondWith', () => {
    const event = makeEvent('https://example.com/api/v1/events?since=42');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('GET to /api/v1/health: does NOT call respondWith', () => {
    const event = makeEvent('https://example.com/api/v1/health');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('GET to /api/v1/health with query string: does NOT call respondWith', () => {
    const event = makeEvent('https://example.com/api/v1/health?probe=1');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('GET to /api/v1/health?sw-fetch-probe=1: forwards through respondWith', () => {
    mockFetch.mockResolvedValue(new Response('ok'));
    const event = makeEvent('https://example.com/api/v1/health?sw-fetch-probe=1');
    handlers.fetch(event);
    expect(event.respondWith).toHaveBeenCalledTimes(1);
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });

  it('GET to a path merely starting with health: bypasses the worker', () => {
    mockFetch.mockResolvedValue(new Response('ok'));
    const event = makeEvent('https://example.com/api/v1/health-history');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('cross-origin GET: does NOT call respondWith', () => {
    const event = makeEvent('https://other.com/api/v1/foo');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('non-API GET (static asset): does NOT call respondWith', () => {
    const event = makeEvent('https://example.com/index.html');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  function dataImageEvent(url: string) {
    return {
      request: { url, method: 'GET', destination: 'image' },
      respondWith: vi.fn(),
    };
  }

  it('GET /data/ image: fetched explicitly, with the retry', async () => {
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce(new Response('png-bytes'));
    const event = dataImageEvent('https://example.com/data/artifacts/ui/shot.png');
    handlers.fetch(event);
    expect(event.respondWith).toHaveBeenCalledTimes(1);
    const response = await event.respondWith.mock.calls[0][0];
    expect(mockFetch).toHaveBeenCalledTimes(2);
    expect(await response.text()).toBe('png-bytes');
  });

  function pngResponse(body: string, declaredLength: number = body.length) {
    return new Response(body, {
      headers: { 'content-type': 'image/png', 'content-length': String(declaredLength) },
    });
  }

  it('GET /data/ image: a body shorter than its Content-Length is fetched again', async () => {
    mockFetch
      .mockResolvedValueOnce(pngResponse('png', 9))
      .mockResolvedValueOnce(pngResponse('png-bytes'));
    const event = dataImageEvent('https://example.com/data/artifacts/ui/shot.png');
    handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(mockFetch).toHaveBeenCalledTimes(2);
    expect(mockFetch.mock.calls[0][1].cache).toBeUndefined();
    expect(mockFetch.mock.calls[1][1].cache).toBe('reload');
    expect(await response.text()).toBe('png-bytes');
    expect(response.headers.get('content-type')).toBe('image/png');
    expect(response.headers.get('content-length')).toBe('9');
  });

  it('GET /data/ image: short on every attempt answers a network error, so the page retries', async () => {
    mockFetch.mockImplementation(() => Promise.resolve(pngResponse('png', 9)));
    const event = dataImageEvent('https://example.com/data/artifacts/ui/shot.png');
    handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(response.type).toBe('error');
  });

  it('GET /data/ image: a body that stalls is abandoned and fetched again', async () => {
    vi.useFakeTimers();
    try {
      mockFetch
        .mockImplementationOnce((_req: unknown, init?: { signal?: AbortSignal }) => {
          const stalled = new ReadableStream({
            start(controller) {
              controller.enqueue(new TextEncoder().encode('png'));
              init?.signal?.addEventListener('abort', () => controller.error(new Error('aborted')));
            },
          });
          return Promise.resolve(new Response(stalled, { headers: { 'content-length': '9' } }));
        })
        .mockResolvedValueOnce(pngResponse('png-bytes'));
      const event = dataImageEvent('https://example.com/data/artifacts/ui/shot.png');
      handlers.fetch(event);
      const pending = event.respondWith.mock.calls[0][0];
      await vi.advanceTimersByTimeAsync(60_000);
      const response = await pending;
      expect(mockFetch).toHaveBeenCalledTimes(2);
      expect(await response.text()).toBe('png-bytes');
    } finally {
      vi.useRealTimers();
    }
  });

  it('GET /data/ image: a non-2xx answer passes through untouched', async () => {
    mockFetch.mockResolvedValueOnce(new Response('gone', { status: 404 }));
    const event = dataImageEvent('https://example.com/data/artifacts/ui/missing.png');
    handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(mockFetch).toHaveBeenCalledTimes(1);
    expect(response.status).toBe(404);
  });

  it('GET /data/ image with a retry param: still fetched explicitly', () => {
    const event = dataImageEvent('https://example.com/data/artifacts/ui/shot.png?retry=2');
    handlers.fetch(event);
    expect(event.respondWith).toHaveBeenCalledTimes(1);
  });

  it('GET /data/ non-image (a video range request, a download): left to the browser', () => {
    const event = makeEvent('https://example.com/data/artifacts/clip.mp4');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('GET image outside /data/: left to the browser', () => {
    const event = dataImageEvent('https://example.com/favicon.svg');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('GET retries once if first fetch throws (covers iOS SW restart race)', async () => {
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce(new Response('ok'));
    const event = makeEvent('https://example.com/api/v1/health?sw-fetch-probe=1');
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
    const event = makeEvent('https://example.com/api/v1/health?sw-fetch-probe=1');
    handlers.fetch(event);
    await expect(event.respondWith.mock.calls[0][0]).rejects.toThrow('Load failed');
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });

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

  it('GET /api/v1/threads/abc/events bypasses worker fetch and cache', () => {
    const event = makeEvent('https://example.com/api/v1/threads/abc/events');
    handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
    expect(mockFetch).not.toHaveBeenCalled();
    expect(mockCache.match).not.toHaveBeenCalled();
  });
});

describe('Service Worker fetch handler — immutable /assets bundle caching', () => {
  it('GET /assets/<hash>.js: serves from Cache API on hit (no network), via SHELL_CACHE', async () => {
    const sw = loadSw();
    const cached = new Response('cached-bundle');
    sw.cacheStore.set('https://example.com/assets/index-abc123.js', cached);
    const event = makeEvent('https://example.com/assets/index-abc123.js');
    sw.handlers.fetch(event);
    expect(event.respondWith).toHaveBeenCalledTimes(1);
    const response = await event.respondWith.mock.calls[0][0];
    expect(response).toBe(cached);
    expect(sw.mockFetch).not.toHaveBeenCalled();
    expect(sw.mockCaches.open).toHaveBeenCalledWith(SHELL_CACHE);
  });

  it('GET /assets/<hash>.css: caches a successful response on miss', async () => {
    const sw = loadSw();
    sw.mockFetch.mockResolvedValue(new Response('fresh', { status: 200 }));
    const event = makeEvent('https://example.com/assets/index-def456.css');
    sw.handlers.fetch(event);
    await event.respondWith.mock.calls[0][0];
    expect(sw.mockFetch).toHaveBeenCalledTimes(1);
    expect(sw.mockCache.put).toHaveBeenCalledTimes(1);
    const cachedReq = sw.mockCache.put.mock.calls[0][0] as { url: string };
    expect(cachedReq.url).toBe('https://example.com/assets/index-def456.css');
  });

  it('GET /assets/<hash>.js: does NOT cache a failed response (404/5xx during a deploy swap)', async () => {
    const sw = loadSw();
    sw.mockFetch.mockResolvedValue(new Response('gone', { status: 404 }));
    const event = makeEvent('https://example.com/assets/missing-000000.js');
    sw.handlers.fetch(event);
    await event.respondWith.mock.calls[0][0];
    expect(sw.mockCache.put).not.toHaveBeenCalled();
  });

  function clientLogBodies(sw: ReturnType<typeof loadSw>) {
    return sw.mockFetch.mock.calls
      .filter(([url]) => String(url).endsWith('/api/v1/internal/client-log'))
      .map(([, init]) => JSON.parse((init as { body: string }).body));
  }

  it('GET /assets/<hash>.js: logs a bundle it could not serve, naming it', async () => {
    const sw = loadSw();
    sw.mockFetch.mockImplementation((url: unknown) => Promise.resolve(
      String(url).endsWith('/client-log') ? new Response(null, { status: 204 }) : new Response('gone', { status: 404 }),
    ));
    const event = makeEvent('https://example.com/assets/missing-000000.js');
    sw.handlers.fetch(event);
    expect((await event.respondWith.mock.calls[0][0]).status).toBe(404);
    expect(event.waitUntil).toHaveBeenCalledTimes(1);
    await event.waitUntil.mock.calls[0][0];
    expect(sw.mockFetch).toHaveBeenCalledWith(expect.stringContaining('/internal/client-log'),
      expect.objectContaining({ headers: expect.objectContaining({ 'x-lucidos-restore-generation': 'unknown' }) }));
    expect(clientLogBodies(sw)).toEqual([{
      category: 'sw',
      code: 'asset_failed',
      message: 'asset_failed',
      data: { path: '/assets/missing-000000.js', status: 404, type: 'default', contentType: 'text/plain;charset=UTF-8' },
    }]);
  });

  it('GET /assets/<hash>.js: logs nothing for a bundle it served', async () => {
    const sw = loadSw();
    sw.mockFetch.mockResolvedValue(new Response('export {};', { headers: { 'content-type': 'text/javascript' } }));
    const event = makeEvent('https://example.com/assets/App-abc123.js');
    sw.handlers.fetch(event);
    await event.respondWith.mock.calls[0][0];
    await event.waitUntil.mock.calls[0][0];
    expect(clientLogBodies(sw)).toEqual([]);
  });

  it('GET /assets/<hash>.js: does NOT cache an HTML SPA-fallback body (deleted bundle)', async () => {
    const sw = loadSw();
    const html = new Response('<!doctype html><html></html>', {
      status: 200,
      headers: { 'content-type': 'text/html; charset=utf-8' },
    });
    sw.mockFetch.mockResolvedValue(html);
    const event = makeEvent('https://example.com/assets/index-OLDHASH.js');
    sw.handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(await response.text()).toBe('<!doctype html><html></html>');
    expect(sw.mockCache.put).not.toHaveBeenCalled();
  });

  function bundleResponse(body: string, declaredLength: number = body.length) {
    return new Response(body, {
      headers: { 'content-type': 'text/javascript', 'content-length': String(declaredLength) },
    });
  }

  it('GET /assets/<hash>.js: a body shorter than its Content-Length is fetched again, and only the whole one cached', async () => {
    const sw = loadSw();
    sw.mockFetch
      .mockResolvedValueOnce(bundleResponse('expo', 12))
      .mockResolvedValueOnce(bundleResponse('export {};//'));
    const event = makeEvent('https://example.com/assets/App-abc123.js');
    sw.handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(sw.mockFetch).toHaveBeenCalledTimes(2);
    expect(sw.mockFetch.mock.calls[1][1].cache).toBe('reload');
    expect(await response.text()).toBe('export {};//');
    expect(sw.mockCache.put).toHaveBeenCalledTimes(1);
    expect(await sw.cacheStore.get('https://example.com/assets/App-abc123.js')!.text())
      .toBe('export {};//');
  });

  it('GET /assets/<hash>.js: short on every attempt is never cached, and answers a network error', async () => {
    const sw = loadSw();
    sw.mockFetch.mockImplementation(() => Promise.resolve(bundleResponse('expo', 12)));
    const event = makeEvent('https://example.com/assets/App-abc123.js');
    sw.handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(response.type).toBe('error');
    expect(sw.mockCache.put).not.toHaveBeenCalled();
  });

  it('GET /assets/<hash>.js: an encoded body is accepted and cached', async () => {
    const sw = loadSw();
    sw.mockFetch.mockResolvedValue(new Response('export {};//', {
      headers: { 'content-type': 'text/javascript', 'content-encoding': 'br', 'content-length': '5' },
    }));
    const event = makeEvent('https://example.com/assets/App-abc123.js');
    sw.handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(sw.mockFetch).toHaveBeenCalledTimes(1);
    expect(await response.text()).toBe('export {};//');
    expect(sw.mockCache.put).toHaveBeenCalledTimes(1);
  });

  it('GET /src/main.tsx (Vite dev module): does NOT call respondWith (no caching in dev)', () => {
    const sw = loadSw();
    const event = makeEvent('https://example.com/src/main.tsx');
    sw.handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('GET /@vite/client (Vite dev runtime): does NOT call respondWith', () => {
    const sw = loadSw();
    const event = makeEvent('https://example.com/@vite/client');
    sw.handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('cross-origin /assets/ GET (CDN): does NOT call respondWith', () => {
    const sw = loadSw();
    const event = makeEvent('https://cdn.other.com/assets/index-abc123.js');
    sw.handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('non-GET /assets/ request: does NOT call respondWith', () => {
    const sw = loadSw();
    const event = makeEvent('https://example.com/assets/index-abc123.js', 'POST');
    sw.handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });
});

const STAMPED_BUILD = 'testbuild0001';
const STAMPED_SHELL_CACHE = `lucidos-shell-${STAMPED_BUILD}`;

describe('Service Worker fetch handler — navigation shell (network-first)', () => {
  it('dev (un-stamped): navigate to / falls through — shell stays network-fresh', () => {
    const sw = loadSw();
    const event = makeEvent('https://example.com/', 'GET', 'navigate');
    sw.handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('built: navigate to /?notification=… fetches a FRESH shell (network-first), even when one is cached', async () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    const stale = new Response('<!doctype html>STALE shell');
    sw.cacheStore.set('https://example.com/', stale);
    const fresh = new Response('<!doctype html>FRESH shell', { status: 200 });
    sw.mockFetch.mockResolvedValue(fresh);
    const event = makeEvent('https://example.com/?notification=nid-1&thread=tid-1', 'GET', 'navigate');
    sw.handlers.fetch(event);
    expect(event.respondWith).toHaveBeenCalledTimes(1);
    const response = await event.respondWith.mock.calls[0][0];
    expect(response).toBe(fresh);
    expect(sw.mockFetch).toHaveBeenCalledTimes(1);
    expect(sw.mockCaches.open).toHaveBeenCalledWith(STAMPED_SHELL_CACHE);
  });

  it('built: a back/forward navigation revalidates the shell instead of taking the HTTP cache copy', async () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    sw.mockFetch.mockResolvedValue(new Response('fresh shell', { status: 200 }));
    const event = makeEvent('https://example.com/', 'GET', 'navigate');
    Object.assign(event.request, { cache: 'force-cache' });
    sw.handlers.fetch(event);
    await event.respondWith.mock.calls[0][0];
    expect(sw.mockFetch).toHaveBeenCalledTimes(1);
    expect(sw.mockFetch.mock.calls[0][1]).toEqual({ cache: 'no-cache' });
  });

  it('built: navigate caches the fresh shell under the normalized / key (query stripped)', async () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    sw.mockFetch.mockResolvedValue(new Response('fresh shell', { status: 200 }));
    const event = makeEvent('https://example.com/?notification=nid-2', 'GET', 'navigate');
    sw.handlers.fetch(event);
    await event.respondWith.mock.calls[0][0];
    expect(sw.mockFetch).toHaveBeenCalledTimes(1);
    expect(sw.mockCache.put).toHaveBeenCalledTimes(1);
    const cachedReq = sw.mockCache.put.mock.calls[0][0] as { url: string };
    expect(cachedReq.url).toBe('https://example.com/');
  });

  it('built: offline (fetch fails) falls back to the cached shell', async () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    const cached = new Response('<!doctype html>offline shell');
    sw.cacheStore.set('https://example.com/', cached);
    sw.mockFetch.mockRejectedValue(new TypeError('Load failed'));
    const event = makeEvent('https://example.com/?notification=nid-3', 'GET', 'navigate');
    sw.handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(response).toBe(cached);
  });

  it('built: navigate to an app-UI iframe (/app/<id>/) is NOT treated as the shell', () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    const event = makeEvent('https://example.com/app/habit-tracker/', 'GET', 'navigate');
    sw.handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('built: a non-navigation GET to / falls through (only navigations hit the shell path)', () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    const event = makeEvent('https://example.com/', 'GET');
    sw.handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('built: navigate does NOT cache a failed shell response, and serves the cached shell instead (502 mid-restart)', async () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    const cached = new Response('<!doctype html>good shell');
    sw.cacheStore.set('https://example.com/', cached);
    sw.mockFetch.mockResolvedValue(new Response('bad gateway', { status: 502 }));
    const event = makeEvent('https://example.com/', 'GET', 'navigate');
    sw.handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(sw.mockCache.put).not.toHaveBeenCalled();
    expect(response).toBe(cached);
  });

  it('built: navigate with no cached shell returns the network response as-is (502 with empty cache)', async () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    const bad = new Response('bad gateway', { status: 502 });
    sw.mockFetch.mockResolvedValue(bad);
    const event = makeEvent('https://example.com/', 'GET', 'navigate');
    sw.handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(sw.mockCache.put).not.toHaveBeenCalled();
    expect(response).toBe(bad);
  });

  it('built: navigate to a stopped/booting workspace shows the gateway 503 boot splash, NOT the cached shell (marker header present)', async () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    const cached = new Response('<!doctype html>good shell');
    sw.cacheStore.set('https://example.com/', cached);
    const splash = new Response('<!doctype html>workspace starting…', {
      status: 503,
      headers: { 'x-lucidos-boot-splash': '1' },
    });
    sw.mockFetch.mockResolvedValue(splash);
    const event = makeEvent('https://example.com/', 'GET', 'navigate');
    sw.handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(sw.mockCache.put).not.toHaveBeenCalled();
    expect(response).toBe(splash);
  });

  it('built: navigate gets any 503 as-is even WITHOUT the marker header (Apply-stale gateway omitting it)', async () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    const cached = new Response('<!doctype html>good shell');
    sw.cacheStore.set('https://example.com/', cached);
    const splash = new Response('<!doctype html>workspace starting…', { status: 503 });
    sw.mockFetch.mockResolvedValue(splash);
    const event = makeEvent('https://example.com/', 'GET', 'navigate');
    sw.handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(sw.mockCache.put).not.toHaveBeenCalled();
    expect(response).toBe(splash);
  });

  it('built: an unpaired navigation shows the pairing screen and never pins it as the shell', async () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    const cached = new Response('<!doctype html>good shell');
    sw.cacheStore.set('https://example.com/', cached);
    const pairing = new Response('<!doctype html>pair this device', {
      status: 200,
      headers: { 'x-lucidos-pairing': '1' },
    });
    sw.mockFetch.mockResolvedValue(pairing);
    const event = makeEvent('https://example.com/', 'GET', 'navigate');
    sw.handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(response).toBe(pairing);
    expect(sw.mockCache.put).not.toHaveBeenCalled();
  });

  it('built: a redirected navigation hands the browser a real redirect, not the cached shell', async () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    sw.cacheStore.set('https://example.com/', new Response('<!doctype html>good shell'));
    const followed = new Response('<!doctype html>somewhere else', { status: 200 });
    Object.defineProperty(followed, 'redirected', { value: true });
    Object.defineProperty(followed, 'url', { value: 'https://example.com/~/' });
    sw.mockFetch.mockResolvedValue(followed);
    const event = makeEvent('https://example.com/', 'GET', 'navigate');
    sw.handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(sw.mockCache.put).not.toHaveBeenCalled();
    expect(response.status).toBe(302);
    expect(response.headers.get('location')).toBe('https://example.com/~/');
  });
});

describe('Service Worker install handler — shell precache', () => {
  function makeInstallEvent() {
    const waiting: Array<Promise<unknown>> = [];
    return { waitUntil: (p: Promise<unknown>) => { waiting.push(p); }, waiting };
  }

  it('built: precaches the shell with cache:reload, keyed by /, and still skipWaiting()s', async () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    sw.mockFetch.mockResolvedValue(new Response('shell', { status: 200 }));
    const event = makeInstallEvent();
    sw.handlers.install(event);
    await Promise.all(event.waiting);
    expect(sw.skipWaiting).toHaveBeenCalledTimes(1);
    expect(sw.mockFetch).toHaveBeenCalledWith(
      'https://example.com/',
      expect.objectContaining({ cache: 'reload' }),
    );
    expect(sw.mockCache.put).toHaveBeenCalledTimes(1);
    expect((sw.mockCache.put.mock.calls[0][0] as { url: string }).url).toBe('https://example.com/');
  });

  it('built: installing while unpaired does NOT freeze the pairing screen in as the offline shell', async () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    sw.mockFetch.mockResolvedValue(
      new Response('<!doctype html>pair this device', {
        status: 200,
        headers: { 'x-lucidos-pairing': '1' },
      }),
    );
    const event = makeInstallEvent();
    sw.handlers.install(event);
    await Promise.all(event.waiting);
    expect(sw.mockCache.put).not.toHaveBeenCalled();
  });

  it('built: a failed precache fetch does not throw or cache (best-effort)', async () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    sw.mockFetch.mockRejectedValue(new TypeError('Load failed'));
    const event = makeInstallEvent();
    sw.handlers.install(event);
    await expect(Promise.all(event.waiting)).resolves.toBeDefined();
    expect(sw.mockCache.put).not.toHaveBeenCalled();
  });

  it('dev (un-stamped): install does NOT precache — shell stays network-fresh', async () => {
    const sw = loadSw();
    const event = makeInstallEvent();
    sw.handlers.install(event);
    await Promise.all(event.waiting);
    expect(sw.skipWaiting).toHaveBeenCalledTimes(1);
    expect(sw.mockFetch).not.toHaveBeenCalled();
  });
});

describe('Service Worker activate handler — cache lifecycle', () => {
  function makeActivateEvent() {
    const waiting: Array<Promise<unknown>> = [];
    return { waitUntil: (p: Promise<unknown>) => { waiting.push(p); }, waiting };
  }

  it('prunes caches outside the keep-list, retains blob + shell caches', async () => {
    const sw = loadSw();
    sw.cacheNames.add('lucidos-blob-v1');
    sw.cacheNames.add(SHELL_CACHE);
    sw.cacheNames.add('lucidos-shell-oldbuild');
    sw.cacheNames.add('some-other-cache');
    const event = makeActivateEvent();
    sw.handlers.activate(event);
    await Promise.all(event.waiting);
    expect(sw.mockCaches.delete).toHaveBeenCalledWith('lucidos-shell-oldbuild');
    expect(sw.mockCaches.delete).toHaveBeenCalledWith('some-other-cache');
    expect(sw.mockCaches.delete).not.toHaveBeenCalledWith('lucidos-blob-v1');
    expect(sw.mockCaches.delete).not.toHaveBeenCalledWith(SHELL_CACHE);
    expect([...sw.cacheNames].sort()).toEqual(['lucidos-blob-v1', SHELL_CACHE].sort());
  });
});

function makePushEvent(payload: unknown) {
  const waiting: Promise<unknown>[] = [];
  return {
    data: {
      json: () => payload,
      text: () => JSON.stringify(payload),
    },
    waitUntil: (p: Promise<unknown>) => {
      waiting.push(p);
    },
    waiting,
  };
}

describe('Service Worker push handler — normal show-notification path', () => {
  let handlers: Record<string, (event: any) => void>;
  let mockRegistration: ReturnType<typeof loadSw>['mockRegistration'];

  beforeEach(() => {
    const sw = loadSw();
    handlers = sw.handlers;
    mockRegistration = sw.mockRegistration;
  });

  it('normal payload triggers showNotification with title/body/tag', async () => {
    const event = makePushEvent({
      web_push: 8030,
      notification: { title: 'Hi', body: 'There', tag: 'notif-show' },
    });
    handlers.push(event);
    await Promise.all(event.waiting);

    expect(mockRegistration.showNotification).toHaveBeenCalledTimes(1);
    const [title, opts] = mockRegistration.showNotification.mock.calls[0];
    expect(title).toBe('Hi');
    expect(opts.tag).toBe('notif-show');
    expect(opts.actions).toEqual([]);
  });

  it('carries the engine\'s action buttons onto the notification', async () => {
    const actions = [{ action: 'unwatch', title: 'Unwatch' }];
    const event = makePushEvent({
      web_push: 8030,
      notification: { title: 'Sam', body: 'Kim, can you look?', tag: 'user-tid-1', actions },
    });
    handlers.push(event);
    await Promise.all(event.waiting);

    const [, opts] = mockRegistration.showNotification.mock.calls[0];
    expect(opts.actions).toEqual(actions);
  });
});

describe('Service Worker message handler — liveness ping', () => {
  let handlers: Record<string, (event: any) => void>;

  beforeEach(() => {
    const sw = loadSw();
    handlers = sw.handlers;
  });

  it('responds to lucidos:ping with lucidos:pong on event.source', () => {
    const source = { postMessage: vi.fn() };
    handlers.message({ data: { type: 'lucidos:ping' }, source });
    expect(source.postMessage).toHaveBeenCalledWith({ type: 'lucidos:pong', instance: expect.any(Number) });
  });

  it('responds to lucidos:get-build-id with the placeholder BUILD_ID (un-stamped dev source)', () => {
    const source = { postMessage: vi.fn() };
    handlers.message({ data: { type: 'lucidos:get-build-id' }, source });
    expect(source.postMessage).toHaveBeenCalledWith({
      type: 'lucidos:build-id',
      buildId: '__LUCIDOS_BUILD_ID__',
    });
  });

  it('responds to lucidos:get-build-id with the stamped BUILD_ID (built source)', () => {
    const sw = loadSw({ buildId: 'testbuild0001' });
    const source = { postMessage: vi.fn() };
    sw.handlers.message({ data: { type: 'lucidos:get-build-id' }, source });
    expect(source.postMessage).toHaveBeenCalledWith({
      type: 'lucidos:build-id',
      buildId: 'testbuild0001',
    });
  });

  it('does not respond to lucidos:get-build-id when event.source is null', () => {
    expect(() => handlers.message({ data: { type: 'lucidos:get-build-id' }, source: null })).not.toThrow();
  });

  it('ignores unknown message types', () => {
    const source = { postMessage: vi.fn() };
    handlers.message({ data: { type: 'something-else' }, source });
    expect(source.postMessage).not.toHaveBeenCalled();
  });

  it('does not throw when event.source is null (e.g. message from a closed client)', () => {
    expect(() => handlers.message({ data: { type: 'lucidos:ping' }, source: null })).not.toThrow();
  });

  it('does not throw on malformed message data', () => {
    const source = { postMessage: vi.fn() };
    expect(() => handlers.message({ data: null, source })).not.toThrow();
    expect(() => handlers.message({ data: undefined, source })).not.toThrow();
    expect(() => handlers.message({ data: 'string', source })).not.toThrow();
    expect(source.postMessage).not.toHaveBeenCalled();
  });
});

describe('Service Worker push handler — userVisibleOnly contract', () => {
  function pushEvent(payload: Record<string, unknown>) {
    const waited: Array<Promise<unknown>> = [];
    return {
      data: { json: () => payload },
      waitUntil: (p: Promise<unknown>) => { waited.push(p); },
      _waited: waited,
    };
  }

  it('calls registration.showNotification on push (no clients connected)', async () => {
    const { handlers, mockRegistration, matchAll } = loadSw();
    matchAll.mockResolvedValue([]);
    const ev = pushEvent({ web_push: 8030, notification: { title: 'Hi', body: 'There', tag: 'nid-1' } });
    handlers.push(ev);
    await Promise.all(ev._waited);
    expect(mockRegistration.showNotification).toHaveBeenCalledTimes(1);
    expect(mockRegistration.showNotification).toHaveBeenCalledWith('Hi', expect.objectContaining({
      body: 'There',
      tag: 'nid-1',
      requireInteraction: true,
    }));
  });

  it('still calls showNotification when a visible client exists (no silent-push shortcut)', async () => {
    const { handlers, mockRegistration, matchAll } = loadSw();
    matchAll.mockResolvedValue([
      { frameType: 'top-level', visibilityState: 'visible', postMessage: vi.fn() },
    ]);
    const ev = pushEvent({ web_push: 8030, notification: { title: 'Hi', body: 'There', tag: 'nid-2' } });
    handlers.push(ev);
    await Promise.all(ev._waited);
    expect(mockRegistration.showNotification).toHaveBeenCalledTimes(1);
  });

  it('falls back to the default tag when the notification has none', async () => {
    const { handlers, mockRegistration } = loadSw();
    const ev = pushEvent({ web_push: 8030, notification: { title: 'Hi', body: 'There' } });
    handlers.push(ev);
    await Promise.all(ev._waited);
    expect(mockRegistration.showNotification).toHaveBeenCalledWith('Hi', expect.objectContaining({
      tag: 'lucidos-notification',
    }));
  });
});

describe('Service Worker push handler — declarative envelope', () => {
  function pushEvent(payload: Record<string, unknown>) {
    const waited: Array<Promise<unknown>> = [];
    return {
      data: { json: () => payload },
      waitUntil: (p: Promise<unknown>) => { waited.push(p); },
      _waited: waited,
    };
  }

  function declarativeEnvelope(notification: Record<string, unknown>, extras: Record<string, unknown> = {}) {
    return {
      web_push: 8030,
      notification,
      ...extras,
    };
  }

  it('declarative push: reads title/body/tag/data from data.notification.*', async () => {
    const { handlers, mockRegistration } = loadSw();
    const ev = pushEvent(declarativeEnvelope({
      title: 'Claude is asking',
      body: 'Reply needed',
      navigate: '/?notification=nid-thread&thread=tid-1&event=evt-7&tap=%7B%22kind%22%3A%22navigate%22%7D',
      tag: 'nid-thread',
      data: {
        notification_id: 'nid-thread',
        thread_id: 'tid-1',
        event_id: 'evt-7',
        navigate: '/#notification=nid-thread&thread=tid-1&event=evt-7&tap=%7B%22kind%22%3A%22navigate%22%7D',
        tap: { kind: 'navigate', to: { target: 'thread', id: 'tid-1', event_id: 'evt-7' } },
      },
    }));
    handlers.push(ev);
    await Promise.all(ev._waited);
    const [title, opts] = mockRegistration.showNotification.mock.calls[0];
    expect(title).toBe('Claude is asking');
    expect(opts.body).toBe('Reply needed');
    expect(opts.tag).toBe('nid-thread');
    expect(opts.navigate).toBe(
      'https://example.com/?notification=nid-thread&thread=tid-1&event=evt-7&tap=%7B%22kind%22%3A%22navigate%22%7D',
    );
  });

  it('declarative push: data block (tap + ids + navigate) round-trips onto opts.data', async () => {
    const { handlers, mockRegistration } = loadSw();
    const ev = pushEvent(declarativeEnvelope({
      title: 'T',
      body: 'B',
      navigate: '/?notification=nid-1',
      tag: 'nid-1',
      data: {
        notification_id: 'nid-1',
        navigate: '/#notification=nid-1',
        tap: { kind: 'navigate', to: { target: 'changes' } },
      },
    }));
    handlers.push(ev);
    await Promise.all(ev._waited);
    const [, opts] = mockRegistration.showNotification.mock.calls[0];
    expect(opts.data).toEqual({
      notification_id: 'nid-1',
      navigate: '/#notification=nid-1',
      tap: { kind: 'navigate', to: { target: 'changes' } },
    });
  });

  it('declarative push: app_badge sets the app-icon badge', async () => {
    const { handlers, setAppBadge, clearAppBadge } = loadSw();
    const ev = pushEvent(
      declarativeEnvelope({ title: 'T', body: 'B', tag: 'nid-1', data: {} }, { app_badge: 3 }),
    );
    handlers.push(ev);
    await Promise.all(ev._waited);
    expect(setAppBadge).toHaveBeenCalledWith(3);
    expect(clearAppBadge).not.toHaveBeenCalled();
  });

  it('declarative push: app_badge 0 clears the app-icon badge', async () => {
    const { handlers, setAppBadge, clearAppBadge } = loadSw();
    const ev = pushEvent(
      declarativeEnvelope({ title: 'T', body: 'B', tag: 'nid-1', data: {} }, { app_badge: 0 }),
    );
    handlers.push(ev);
    await Promise.all(ev._waited);
    expect(clearAppBadge).toHaveBeenCalledTimes(1);
    expect(setAppBadge).not.toHaveBeenCalled();
  });

  it('declarative push: no app_badge field leaves the badge untouched', async () => {
    const { handlers, setAppBadge, clearAppBadge } = loadSw();
    const ev = pushEvent(declarativeEnvelope({ title: 'T', body: 'B', tag: 'nid-1', data: {} }));
    handlers.push(ev);
    await Promise.all(ev._waited);
    expect(setAppBadge).not.toHaveBeenCalled();
    expect(clearAppBadge).not.toHaveBeenCalled();
  });

  it('declarative push: missing navigate falls back to bare origin', async () => {
    const { handlers, mockRegistration } = loadSw();
    const ev = pushEvent(declarativeEnvelope({
      title: 'Bare',
      body: 'No nav',
      tag: 'lucidos-notification',
      data: {},
    }));
    handlers.push(ev);
    await Promise.all(ev._waited);
    const [, opts] = mockRegistration.showNotification.mock.calls[0];
    expect(opts.navigate).toBe('https://example.com/');
  });

  it('a payload that is not declarative shows the generic notification', async () => {
    const { handlers, mockRegistration } = loadSw();
    const ev = pushEvent({ title: 'Flat', body: 'Old shape', notification_id: 'nid-flat' });
    handlers.push(ev);
    await Promise.all(ev._waited);
    const [title, opts] = mockRegistration.showNotification.mock.calls[0];
    expect(title).toBe('Lucidos');
    expect(opts.body).toBe('New notification');
    expect(opts.tag).toBe('lucidos-notification');
    expect(opts.navigate).toBe('https://example.com/');
    expect(opts.data).toEqual({});
  });

  it('non-JSON payload: defaults to Lucidos title + text body', async () => {
    const { handlers, mockRegistration } = loadSw();
    const ev = {
      data: {
        json: () => { throw new Error('not json'); },
        text: () => 'fallback body',
      },
      waitUntil: (p: Promise<unknown>) => { (ev as { _waited: Promise<unknown>[] })._waited.push(p); },
      _waited: [] as Promise<unknown>[],
    };
    handlers.push(ev);
    await Promise.all((ev as { _waited: Promise<unknown>[] })._waited);
    const [title, opts] = mockRegistration.showNotification.mock.calls[0];
    expect(title).toBe('Lucidos');
    expect(opts.body).toBe('fallback body');
  });
});

describe('Service Worker push handler — wake variant (layer 3)', () => {
  function pushEvent(payload: Record<string, unknown>) {
    const waited: Array<Promise<unknown>> = [];
    return {
      data: { json: () => payload },
      waitUntil: (p: Promise<unknown>) => { waited.push(p); },
      _waited: waited,
    };
  }

  it('wake:true push still calls showNotification (Chrome silent-push budget)', async () => {
    const { handlers, mockRegistration } = loadSw();
    const ev = pushEvent({
      web_push: 8030,
      notification: {
        title: 'Claude is asking',
        body: 'Pick one',
        tag: 'nid-stuck',
        data: { notification_id: 'nid-stuck', tap: { kind: 'navigate', to: { target: 'thread', id: 'tid-stuck' } } },
      },
      wake: true,
    });
    handlers.push(ev);
    await Promise.all(ev._waited);
    expect(mockRegistration.showNotification).toHaveBeenCalledTimes(1);
  });

  it('wake:true push sets renotify:false and silent:true (no re-pop, no sound)', async () => {
    const { handlers, mockRegistration } = loadSw();
    const ev = pushEvent({
      web_push: 8030, notification: { title: 'T', body: 'B', tag: 'nid-stuck' }, wake: true,
    });
    handlers.push(ev);
    await Promise.all(ev._waited);
    const [, opts] = mockRegistration.showNotification.mock.calls[0];
    expect(opts.renotify).toBe(false);
    expect(opts.silent).toBe(true);
    expect(opts.requireInteraction).toBe(true);
    expect(opts.tag).toBe('nid-stuck');
  });

  it('non-wake push keeps renotify:true and silent:false (original behavior)', async () => {
    const { handlers, mockRegistration } = loadSw();
    const ev = pushEvent({ web_push: 8030, notification: { title: 'T', body: 'B', tag: 'nid-real' } });
    handlers.push(ev);
    await Promise.all(ev._waited);
    const [, opts] = mockRegistration.showNotification.mock.calls[0];
    expect(opts.renotify).toBe(true);
    expect(opts.silent).toBe(false);
  });
});

describe('Service Worker notificationclick handler — deep-link routing', () => {
  function makeClickEvent(data: Record<string, unknown>) {
    const waited: Array<Promise<unknown>> = [];
    return {
      notification: { data, close: vi.fn() },
      waitUntil: (p: Promise<unknown>) => { waited.push(p); },
      _waited: waited,
    };
  }

  function topLevelClient(url = 'https://example.com/') {
    return {
      frameType: 'top-level',
      visibilityState: 'visible',
      url,
      navigate: vi.fn(() => Promise.resolve({ focus: vi.fn(() => Promise.resolve()) })),
      focus: vi.fn(() => Promise.resolve()),
      postMessage: vi.fn(),
    };
  }

  const threadData = {
    notification_id: 'nid-thread',
    thread_id: 'tid-1',
    event_id: 'evt-7',
    tap: { kind: 'navigate', to: { target: 'thread', id: 'tid-1', event_id: 'evt-7' } },
    navigate: '/#notification=nid-thread&thread=tid-1&event=evt-7',
  };

  it('warm controlled tab: posts the deep link to the existing client (not a fragment navigate)', async () => {
    const { handlers, matchAll, mockFetch, openWindow } = loadSw();
    mockFetch.mockResolvedValue(new Response('ok'));
    const client = topLevelClient();
    matchAll.mockResolvedValue([client]);

    const ev = makeClickEvent(threadData);
    handlers.notificationclick(ev);
    await Promise.all(ev._waited);

    expect(client.postMessage).toHaveBeenCalledWith({ type: 'lucidos:deep-link', target: threadData });
    expect(client.focus).toHaveBeenCalledTimes(1);
    expect(client.navigate).not.toHaveBeenCalled();
    expect(openWindow).not.toHaveBeenCalled();
  });

  it('Unwatch leaves the thread and marks the notification read, opening nothing', async () => {
    const { handlers, matchAll, mockFetch, openWindow } = loadSw();
    mockFetch.mockResolvedValue(new Response('ok'));
    const client = topLevelClient();
    matchAll.mockResolvedValue([client]);

    const ev = { ...makeClickEvent({ ...threadData, unwatch_thread_id: 'tid-1' }), action: 'unwatch' };
    handlers.notificationclick(ev);
    await Promise.all(ev._waited);

    expect(mockFetch).toHaveBeenCalledWith(
      'https://example.com/api/v1/threads/tid-1/users/unwatch',
      expect.objectContaining({ method: 'POST' }),
    );
    expect(mockFetch).toHaveBeenCalledWith(
      'https://example.com/api/v1/notification/read?id=nid-thread',
      expect.objectContaining({ method: 'POST' }),
    );
    expect(client.postMessage).not.toHaveBeenCalled();
    expect(openWindow).not.toHaveBeenCalled();
  });

  it('marks the source notification read via fetch (modal-tap: read even when page-side dispatch is the modal)', async () => {
    const { handlers, matchAll, mockFetch } = loadSw();
    mockFetch.mockResolvedValue(new Response('ok'));
    matchAll.mockResolvedValue([topLevelClient()]);

    const modalData = { notification_id: 'nid-modal', tap: { kind: 'modal' }, navigate: '/#notification=nid-modal' };
    const ev = makeClickEvent(modalData);
    handlers.notificationclick(ev);
    await Promise.all(ev._waited);

    expect(mockFetch).toHaveBeenCalledWith(
      'https://example.com/api/v1/notification/read?id=nid-modal',
      expect.objectContaining({ method: 'POST' }),
    );
  });

  it('modal tap posts the deep link to the existing client so the page opens the inbox modal', async () => {
    const { handlers, matchAll, mockFetch } = loadSw();
    mockFetch.mockResolvedValue(new Response('ok'));
    const client = topLevelClient();
    matchAll.mockResolvedValue([client]);

    const modalData = { notification_id: 'nid-modal', tap: { kind: 'modal' }, navigate: '/#notification=nid-modal' };
    const ev = makeClickEvent(modalData);
    handlers.notificationclick(ev);
    await Promise.all(ev._waited);

    expect(client.postMessage).toHaveBeenCalledWith({ type: 'lucidos:deep-link', target: modalData });
  });

  it('no existing tab (cold): opens a window at the engine-built deep-link URL', async () => {
    const { handlers, matchAll, mockFetch, openWindow } = loadSw();
    mockFetch.mockResolvedValue(new Response('ok'));
    matchAll.mockResolvedValue([]);

    const ev = makeClickEvent(threadData);
    handlers.notificationclick(ev);
    await Promise.all(ev._waited);

    expect(openWindow).toHaveBeenCalledWith(
      'https://example.com/#notification=nid-thread&thread=tid-1&event=evt-7',
    );
  });

  it('closes the OS notification on tap', async () => {
    const { handlers, matchAll, mockFetch } = loadSw();
    mockFetch.mockResolvedValue(new Response('ok'));
    matchAll.mockResolvedValue([topLevelClient()]);

    const ev = makeClickEvent(threadData);
    handlers.notificationclick(ev);
    await Promise.all(ev._waited);

    expect(ev.notification.close).toHaveBeenCalledTimes(1);
  });

  const SCOPE_PERSONAL = 'https://example.com/personal/';

  it('cross-workspace: a tab in a DIFFERENT scope is not focused/messaged — opens a new window in THIS scope', async () => {
    const { handlers, matchAll, mockFetch, openWindow } = loadSw({ scope: SCOPE_PERSONAL });
    mockFetch.mockResolvedValue(new Response('ok'));
    const devTab = topLevelClient('https://example.com/dev/');
    matchAll.mockResolvedValue([devTab]);

    const ev = makeClickEvent(threadData);
    handlers.notificationclick(ev);
    await Promise.all(ev._waited);

    expect(devTab.postMessage).not.toHaveBeenCalled();
    expect(devTab.focus).not.toHaveBeenCalled();
    expect(openWindow).toHaveBeenCalledWith(
      'https://example.com/personal/#notification=nid-thread&thread=tid-1&event=evt-7',
    );
  });

  it('cross-workspace: posts the deep link only to the SAME-scope tab when both are open', async () => {
    const { handlers, matchAll, mockFetch, openWindow } = loadSw({ scope: SCOPE_PERSONAL });
    mockFetch.mockResolvedValue(new Response('ok'));
    const devTab = topLevelClient('https://example.com/dev/');
    const personalTab = topLevelClient('https://example.com/personal/');
    matchAll.mockResolvedValue([devTab, personalTab]);

    const ev = makeClickEvent(threadData);
    handlers.notificationclick(ev);
    await Promise.all(ev._waited);

    expect(personalTab.postMessage).toHaveBeenCalledWith({ type: 'lucidos:deep-link', target: threadData });
    expect(personalTab.focus).toHaveBeenCalledTimes(1);
    expect(devTab.postMessage).not.toHaveBeenCalled();
    expect(openWindow).not.toHaveBeenCalled();
  });

  const SCOPE_DEV = 'https://example.com/dev/';
  const APP_TAB = 'https://example.com/dev/app/habit-tracker/';

  it('popped-out app tab: the shell is the tap target, never the app tab', async () => {
    const { handlers, matchAll, mockFetch, openWindow } = loadSw({ scope: SCOPE_DEV });
    mockFetch.mockResolvedValue(new Response('ok'));
    const appTab = topLevelClient(APP_TAB);
    const shell = topLevelClient(SCOPE_DEV);
    matchAll.mockResolvedValue([appTab, shell]);

    const ev = makeClickEvent(threadData);
    handlers.notificationclick(ev);
    await Promise.all(ev._waited);

    expect(shell.postMessage).toHaveBeenCalledWith({ type: 'lucidos:deep-link', target: threadData });
    expect(shell.focus).toHaveBeenCalledTimes(1);
    expect(appTab.postMessage).not.toHaveBeenCalled();
    expect(appTab.focus).not.toHaveBeenCalled();
    expect(openWindow).not.toHaveBeenCalled();
  });

  it('only a popped-out app tab open: opens the shell, leaving the app tab alone', async () => {
    const { handlers, matchAll, mockFetch, openWindow } = loadSw({ scope: SCOPE_DEV });
    mockFetch.mockResolvedValue(new Response('ok'));
    const appTab = topLevelClient(APP_TAB);
    matchAll.mockResolvedValue([appTab]);

    const ev = makeClickEvent(threadData);
    handlers.notificationclick(ev);
    await Promise.all(ev._waited);

    expect(appTab.postMessage).not.toHaveBeenCalled();
    expect(appTab.focus).not.toHaveBeenCalled();
    expect(openWindow).toHaveBeenCalledWith(
      'https://example.com/dev/#notification=nid-thread&thread=tid-1&event=evt-7',
    );
  });

  it('the shell matches on path alone: its own query, hash and bare slug all qualify', async () => {
    for (const url of [
      'https://example.com/dev/?notification=older',
      'https://example.com/dev/#notifications',
      'https://example.com/dev',
    ]) {
      const { handlers, matchAll, mockFetch, openWindow } = loadSw({ scope: SCOPE_DEV });
      mockFetch.mockResolvedValue(new Response('ok'));
      const shell = topLevelClient(url);
      matchAll.mockResolvedValue([shell]);

      const ev = makeClickEvent(threadData);
      handlers.notificationclick(ev);
      await Promise.all(ev._waited);

      expect(shell.postMessage).toHaveBeenCalledWith({ type: 'lucidos:deep-link', target: threadData });
      expect(openWindow).not.toHaveBeenCalled();
    }
  });

  it('marks read via the scoped notification/read endpoint (gateway scope prefix)', async () => {
    const { handlers, matchAll, mockFetch } = loadSw({ scope: SCOPE_PERSONAL });
    mockFetch.mockResolvedValue(new Response('ok'));
    matchAll.mockResolvedValue([topLevelClient('https://example.com/personal/')]);

    const ev = makeClickEvent(threadData);
    handlers.notificationclick(ev);
    await Promise.all(ev._waited);

    expect(mockFetch).toHaveBeenCalledWith(
      'https://example.com/personal/api/v1/notification/read?id=nid-thread',
      expect.objectContaining({ method: 'POST' }),
    );
  });
});


describe('Service Worker base-path awareness (gateway scope)', () => {
  const SCOPE = 'https://example.com/ws/work/';

  it('bypasses /ws/<id>/api/v1 GETs', () => {
    const { handlers, mockFetch } = loadSw({ scope: SCOPE });
    mockFetch.mockResolvedValue(new Response('ok'));
    const ev = makeEvent('https://example.com/ws/work/api/v1/threads/list');
    handlers.fetch(ev);
    expect(ev.respondWith).not.toHaveBeenCalled();
  });

  it('does NOT intercept the scoped SSE stream', () => {
    const { handlers } = loadSw({ scope: SCOPE });
    const ev = makeEvent('https://example.com/ws/work/api/v1/events');
    handlers.fetch(ev);
    expect(ev.respondWith).not.toHaveBeenCalled();
  });

  it('does NOT intercept the scoped health probe', () => {
    const { handlers } = loadSw({ scope: SCOPE });
    const ev = makeEvent('https://example.com/ws/work/api/v1/health');
    handlers.fetch(ev);
    expect(ev.respondWith).not.toHaveBeenCalled();
  });

  it('cache-firsts scoped /assets bundles', () => {
    const { handlers, mockFetch } = loadSw({ scope: SCOPE, buildId: 'abc123def456' });
    mockFetch.mockResolvedValue(new Response('ok', { headers: { 'content-type': 'application/javascript' } }));
    const ev = makeEvent('https://example.com/ws/work/assets/index-deadbeef.js');
    handlers.fetch(ev);
    expect(ev.respondWith).toHaveBeenCalled();
  });

  it('network-firsts the scoped navigation shell (/ws/<id>/)', () => {
    const { handlers, mockFetch } = loadSw({ scope: SCOPE, buildId: 'abc123def456' });
    mockFetch.mockResolvedValue(new Response('<html></html>', { headers: { 'content-type': 'text/html' } }));
    const scopedNav = makeEvent('https://example.com/ws/work/', 'GET', 'navigate');
    handlers.fetch(scopedNav);
    expect(scopedNav.respondWith).toHaveBeenCalled();
  });

  it('renders push notifications with scope-prefixed icons', async () => {
    const { handlers, mockRegistration } = loadSw({ scope: SCOPE });
    await handlers.push({
      data: { json: () => ({ web_push: 8030, notification: { title: 'T', body: 'B' } }) },
      waitUntil: (p: Promise<unknown>) => p,
    });
    const opts = mockRegistration.showNotification.mock.calls[0][1] as Record<string, string>;
    expect(opts.icon).toBe('/ws/work/favicon.svg');
    expect(opts.navigate).toBe('https://example.com/ws/work/');
  });
});

describe('trusted remote push worker', () => {
  const scope = 'https://example.com/work@peer/';

  it('never warms, intercepts or deletes shared caches', async () => {
    const {handlers, mockCaches, mockFetch} = loadSw({scope, buildId: 'trusted-a-build'});
    const waits: Promise<unknown>[] = [];
    const lifecycle = {waitUntil: (promise: Promise<unknown>) => waits.push(promise)};
    handlers.install(lifecycle);
    handlers.activate(lifecycle);
    await Promise.all(waits);
    for (const path of ['', 'assets/client.js', 'api/v1/threads', 'data/report.html']) {
      const event = makeEvent(scope + path, 'GET', 'navigate');
      handlers.fetch(event);
      expect(event.respondWith).not.toHaveBeenCalled();
    }
    expect(mockCaches.keys).not.toHaveBeenCalled();
    expect(mockCaches.open).not.toHaveBeenCalled();
    expect(mockFetch).not.toHaveBeenCalled();
  });

  it('keeps notification destinations inside the selected remote shell', async () => {
    for (const navigate of ['https://other.example/', '/~/api/v1/control/workspaces', '../local/', 'javascript:alert(1)']) {
      const {handlers, mockRegistration} = loadSw({scope});
      const waits: Promise<unknown>[] = [];
      handlers.push({data: {json: () => ({web_push: 8030, notification: {title: 'Remote', navigate}})}, waitUntil: (p: Promise<unknown>) => waits.push(p)});
      await Promise.all(waits);
      expect(mockRegistration.showNotification.mock.calls[0][1].navigate).toBe(scope);
    }
  });
});

describe('Service Worker source generation', () => {
  it.each([
    ['12345678-1234-4234-8234-123456789abc', '12345678-1234-4234-8234-123456789abc'],
    [null, 'none'],
    [undefined, 'unknown'],
  ])('binds read and click log to notification generation %s', async (generation, header) => {
    const { handlers, mockFetch } = loadSw();
    mockFetch.mockResolvedValue(new Response('ok'));
    const waited: Promise<unknown>[] = [];
    const data = { notification_id: 'source', restore_generation: generation };
    handlers.notificationclick({
      notification: { data, close: vi.fn() },
      waitUntil: (p: Promise<unknown>) => waited.push(p),
    });
    data.restore_generation = 'changed-after-dispatch';
    await Promise.all(waited);
    expect(mockFetch).toHaveBeenCalledTimes(2);
    for (const [, options] of mockFetch.mock.calls) {
      expect(options.headers['x-lucidos-restore-generation']).toBe(header);
    }
  });

  it('binds push logging to payload provenance', async () => {
    const { handlers, mockFetch } = loadSw();
    mockFetch.mockResolvedValue(new Response('ok'));
    const waited: Promise<unknown>[] = [];
    handlers.push({
      data: { json: () => ({ web_push: 8030, notification: {
        title: 'Source', data: { restore_generation: '12345678-1234-4234-8234-123456789abc' },
      } }) },
      waitUntil: (p: Promise<unknown>) => waited.push(p),
    });
    await Promise.all(waited);
    expect(mockFetch).toHaveBeenCalledWith(expect.stringContaining('/internal/client-log'),
      expect.objectContaining({ headers: expect.objectContaining({
        'x-lucidos-restore-generation': '12345678-1234-4234-8234-123456789abc',
      }) }));
  });
});
