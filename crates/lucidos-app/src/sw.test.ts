import { describe, it, expect, vi, beforeEach } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const swSource = readFileSync(resolve(__dirname, '../public/sw.js'), 'utf-8');

// In source (and in the live dev server) sw.js carries the literal
// `__LUCIDOS_BUILD_ID__` placeholder; the `lucidos-sw-stamp` Vite plugin
// replaces it with a per-build id at `vite build` time (vite.config.ts). These
// tests run the unstamped source, so the shell cache name keeps the placeholder.
const SHELL_CACHE = 'lucidos-shell-__LUCIDOS_BUILD_ID__';

type FakeClient = {
  frameType: string;
  visibilityState: string;
  url?: string;
  postMessage?: (msg: unknown) => void;
  focus?: () => Promise<unknown>;
  navigate?: (url: string) => Promise<unknown>;
};

// Runs sw.js inside a sandbox where `self`, `fetch`, `clients`, and `caches`
// are mocks. Returns the registered fetch handler so tests can drive it.
// Top-level handlers (push, notificationclick) only register their listeners —
// they don't fire at load time, so the mocks just need to satisfy the
// addEventListener calls.
//
// `opts.buildId` simulates the `lucidos-sw-stamp` plugin: it replaces the
// `__LUCIDOS_BUILD_ID__` placeholder, flipping the SW's `IS_BUILT` gate true so
// the navigation-shell cache (built mode only) is exercised. Omit it to test
// the dev (un-stamped) behavior where the shell stays network-fresh.
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
  // Track which named caches exist so the activate handler's prune
  // (caches.keys() → delete everything not in KEEP_CACHES) is observable.
  const cacheNames = new Set<string>();
  const mockCaches = {
    open: vi.fn((name: string) => { cacheNames.add(name); return Promise.resolve(mockCache); }),
    keys: vi.fn(() => Promise.resolve([...cacheNames])),
    delete: vi.fn((name: string) => Promise.resolve(cacheNames.delete(name))),
  };
  const mockRegistration = {
    showNotification: vi.fn((_title: string, _opts: Record<string, unknown>) => Promise.resolve()),
    // The SW derives SCOPE_PATH from registration.scope (ADR 0013 base-path
    // awareness); root scope keeps the existing root-path assertions valid.
    scope: opts.scope ?? 'https://example.com/',
  };
  // WorkerNavigator with the Badging API — the push handler mirrors the
  // payload's `app_badge` onto the installed PWA icon (see sw.js push handler).
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
  // matchAll satisfies swDebugLog's best-effort broadcast (returns no clients
  // by default); claim satisfies the activate handler. Push / notificationclick
  // tests can override matchAll if they want to assert a particular client shape.
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

// `mode` populates request.mode so navigation-shell tests can mark a request as
// a top-level navigation. Omit it for the asset/API/blob tests (those branch on
// path + method, never mode).
function makeEvent(url: string, method: string = 'GET', mode?: string) {
  return {
    request: { url, method, ...(mode ? { mode } : {}) },
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

  it('GET to /api/v1/foo: calls respondWith (needed for iOS empty-response fix)', () => {
    mockFetch.mockResolvedValue(new Response('ok'));
    const event = makeEvent('https://example.com/api/v1/threads/abc/events');
    handlers.fetch(event);
    expect(event.respondWith).toHaveBeenCalledTimes(1);
  });

  it('POST to /api/v1/foo: does NOT call respondWith (browser handles natively, avoids iOS body-clone bug)', () => {
    const event = makeEvent('https://example.com/api/v1/chat', 'POST');
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

  it('GET retries once if first fetch throws (covers iOS SW restart race)', async () => {
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce(new Response('ok'));
    const event = makeEvent('https://example.com/api/v1/threads/abc/events');
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
    const event = makeEvent('https://example.com/api/v1/threads/abc/events');
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

  it('GET /api/v1/threads/abc/events: still uses fetchWithRetry path (NOT Cache API)', async () => {
    mockFetch.mockResolvedValue(new Response('ok'));
    const event = makeEvent('https://example.com/api/v1/threads/abc/events');
    handlers.fetch(event);
    await event.respondWith.mock.calls[0][0];
    expect(mockCache.put).not.toHaveBeenCalled();
    expect(mockCache.match).not.toHaveBeenCalled();
  });
});

// Content-hashed app bundles (Vite's build output, /assets/<name>-<hash>.<ext>)
// are immutable for a given URL, so the SW serves them Cache-first — a reload
// pulls the JS/CSS graph from disk instead of the network (the bulk of an iOS
// PWA reload after a notification-tap navigation; see notifications.md §4.5).
// The branch self-gates across run modes: the Vite dev server serves modules
// from /src, /@vite, /@id, /node_modules/.vite — never /assets/* — so it never
// fires in dev, where caching unhashed modules would pin stale code / break HMR.
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

  // A bundle deleted by a later `vite build --watch` rebuild resolves through the
  // dev server's SPA fallback to index.html (200 text/html), NOT a 404. Caching
  // that under the bundle URL would poison the entry forever (the page loads HTML
  // as a module script → no JS → black #app). The asset branch must treat an HTML
  // body as a miss: serve it through but never store it.
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
    expect(response).toBe(html); // passed through, not swallowed
    expect(sw.mockCache.put).not.toHaveBeenCalled();
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

// Navigation-shell serving (twelfth iteration, notifications.md §4.5). The shell
// is served NETWORK-FIRST: every top-level navigation — the PWA start URL and
// every notification-tap reload (`/?notification=…`, the cross-document load
// WebKit forces on iOS) — fetches a fresh index.html so the shell always matches
// the content-hashed /assets/* bundles the server currently has. A shell pinned
// cache-first from an earlier build referenced bundles a later `vite build
// --watch` had already deleted; the server's SPA fallback answered those with
// index.html (200 text/html), the page loaded HTML as its entry module script,
// and the PWA went black. The cache is the OFFLINE fallback only. Built mode only
// (the SW's IS_BUILT gate, flipped here by stamping a fake build id); the live
// dev server must serve a network-fresh shell so HMR / index.html edits aren't
// pinned. The cache entry is keyed by the normalized `/` URL so every query
// variant collapses onto one shell; `path === '/'` excludes app-UI iframe
// (`/app/<id>/`) navigations, which are their own server-rendered HTML.
const STAMPED_BUILD = 'testbuild0001';
const STAMPED_SHELL_CACHE = `lucidos-shell-${STAMPED_BUILD}`;

describe('Service Worker fetch handler — navigation shell (network-first)', () => {
  it('dev (un-stamped): navigate to / falls through — shell stays network-fresh', () => {
    const sw = loadSw(); // IS_BUILT false → navigation branch inert
    const event = makeEvent('https://example.com/', 'GET', 'navigate');
    sw.handlers.fetch(event);
    expect(event.respondWith).not.toHaveBeenCalled();
  });

  it('built: navigate to /?notification=… fetches a FRESH shell (network-first), even when one is cached', async () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    // A stale shell is already cached under the normalized `/` key. Network-first
    // must NOT serve it while online — that is exactly the stale-bundle black
    // screen this iteration fixes.
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
    // Both the initial fetch and the fetchWithRetry retry reject.
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
    const event = makeEvent('https://example.com/', 'GET'); // no mode
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
    expect(response).toBe(cached); // prefers the last good shell over the 502
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
    expect(sw.mockCache.put).not.toHaveBeenCalled(); // never cache the 503
    expect(response).toBe(splash); // splash wins over the stale cached shell
  });

  it('built: navigate gets any 503 as-is even WITHOUT the marker header (Apply-stale gateway omitting it)', async () => {
    const sw = loadSw({ buildId: STAMPED_BUILD });
    const cached = new Response('<!doctype html>good shell');
    sw.cacheStore.set('https://example.com/', cached);
    // A gateway predating the X-Lucidos-Boot-Splash marker still answers a
    // stopped/booting workspace with a 503 splash; the engine is down either way,
    // so the cached shell would just 503-storm. Key on the 503 status, not the
    // header, so the splash shows against an older gateway too.
    const splash = new Response('<!doctype html>workspace starting…', { status: 503 });
    sw.mockFetch.mockResolvedValue(splash);
    const event = makeEvent('https://example.com/', 'GET', 'navigate');
    sw.handlers.fetch(event);
    const response = await event.respondWith.mock.calls[0][0];
    expect(sw.mockCache.put).not.toHaveBeenCalled();
    expect(response).toBe(splash);
  });
});

// install precaches the shell (built mode) so an OFFLINE first navigation still
// has an index.html to fall back to. The shell is served network-first, so this
// precache is the offline safety net, not the hot path.
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

// activate() takes control of open pages, then prunes any cache it no longer
// recognizes so a new build's shell cache name (lucidos-shell-<BUILD_ID>)
// purges the prior generation instead of leaking it forever.
describe('Service Worker activate handler — cache lifecycle', () => {
  function makeActivateEvent() {
    const waiting: Array<Promise<unknown>> = [];
    return { waitUntil: (p: Promise<unknown>) => { waiting.push(p); }, waiting };
  }

  it('prunes caches outside the keep-list, retains blob + shell caches', async () => {
    const sw = loadSw();
    sw.cacheNames.add('lucidos-blob-v1');
    sw.cacheNames.add(SHELL_CACHE);
    sw.cacheNames.add('lucidos-shell-oldbuild'); // stale prior-build generation
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

// Build a push event with a JSON payload. The SW handler calls `event.data.json()`
// and `event.waitUntil(promise)` — collect waitUntil promises so tests can await
// them before asserting.
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
  // The cross-device read-marker pushes (type=mark_read / mark_all_read)
  // were removed — see work-tracker `pwa-read-on-another-device-noise`.
  // The push handler now only deals with create-time notifications.
  let handlers: Record<string, (event: any) => void>;
  let mockRegistration: ReturnType<typeof loadSw>['mockRegistration'];

  beforeEach(() => {
    const sw = loadSw();
    handlers = sw.handlers;
    mockRegistration = sw.mockRegistration;
  });

  it('normal payload triggers showNotification with title/body/tag', async () => {
    const event = makePushEvent({
      title: 'Hi',
      body: 'There',
      notification_id: 'notif-show',
    });
    handlers.push(event);
    await Promise.all(event.waiting);

    expect(mockRegistration.showNotification).toHaveBeenCalledTimes(1);
    const [title, opts] = mockRegistration.showNotification.mock.calls[0];
    expect(title).toBe('Hi');
    expect(opts.tag).toBe('notif-show');
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
    expect(source.postMessage).toHaveBeenCalledWith({ type: 'lucidos:pong' });
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

// Locks the always-showNotification contract — see sw.js push handler for the
// Chrome silent-push-budget rationale.
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
    const ev = pushEvent({ title: 'Hi', body: 'There', notification_id: 'nid-1' });
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
    // Even with a visible top-level Lucidos tab the SW must NOT route via
    // postMessage and skip showNotification — that's exactly what burned
    // Chrome's silent-push budget last time.
    matchAll.mockResolvedValue([
      { frameType: 'top-level', visibilityState: 'visible', postMessage: vi.fn() },
    ]);
    const ev = pushEvent({ title: 'Hi', body: 'There', notification_id: 'nid-2' });
    handlers.push(ev);
    await Promise.all(ev._waited);
    expect(mockRegistration.showNotification).toHaveBeenCalledTimes(1);
  });

  it('falls back to the default tag when notification_id is missing', async () => {
    const { handlers, mockRegistration } = loadSw();
    const ev = pushEvent({ title: 'Hi', body: 'There' });
    handlers.push(ev);
    await Promise.all(ev._waited);
    expect(mockRegistration.showNotification).toHaveBeenCalledWith('Hi', expect.objectContaining({
      tag: 'lucidos-notification',
    }));
  });
});

// Declarative Web Push envelope — the wire format the engine emits
// (`{web_push: 8030, notification: {...}}`). Safari 18.5+ handles this
// declaratively (the SW push handler never runs); Chrome/Firefox don't
// recognize the magic so this SW handler parses the envelope manually,
// reads `notification.navigate` straight off the wire (URL built by the
// engine — see crates/lucidos-engine/src/scheduler/push.rs::build_push_payload),
// and stamps it on showNotification. The engine emits TWO navigate forms:
// `notification.navigate` is the QUERY URL (`/?…`, for iOS/declarative), while
// `notification.data.navigate` is the HASH URL (`/#…`, what notificationclick
// reads for `client.navigate()`). See system-knowhow notifications.md §4.5.
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
      // QUERY form — the iOS/declarative navigate URL the engine stamps on
      // `notification.navigate`. The SW copies it to showNotification's
      // `navigate` option (honored by Safari / future declarative Chrome).
      navigate: '/?notification=nid-thread&thread=tid-1&event=evt-7&tap=%7B%22kind%22%3A%22navigate%22%7D',
      tag: 'nid-thread',
      data: {
        notification_id: 'nid-thread',
        thread_id: 'tid-1',
        event_id: 'evt-7',
        // HASH form — what notificationclick reads for client.navigate().
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
    // SW resolves the relative `notification.navigate` (the declarative/iOS
    // query URL) against its own origin so a declarative navigate gets the
    // absolute URL it requires. Safari handles the relative form natively
    // without touching this SW.
    expect(opts.navigate).toBe(
      'https://example.com/?notification=nid-thread&thread=tid-1&event=evt-7&tap=%7B%22kind%22%3A%22navigate%22%7D',
    );
  });

  it('declarative push: data block (tap + ids + navigate) round-trips onto opts.data', async () => {
    // The notificationclick handler reads navigate + notification_id + tap
    // off event.notification.data. Locks the contract that the engine-built
    // data block is what shows up there.
    const { handlers, mockRegistration } = loadSw();
    const ev = pushEvent(declarativeEnvelope({
      title: 'T',
      body: 'B',
      navigate: '/?notification=nid-1', // QUERY form (iOS/declarative)
      tag: 'nid-1',
      data: {
        notification_id: 'nid-1',
        navigate: '/#notification=nid-1', // HASH form (Chrome notificationclick)
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
    // The engine carries the workspace's unread count in the top-level
    // `app_badge` field; the SW mirrors it onto the installed PWA icon so a
    // CLOSED workspace PWA stays accurate on Chrome/Android.
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
    // Defensive: the engine always emits navigate today, but legacy payloads
    // or future engine bugs shouldn't crash the SW.
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

  it('legacy flat-shape push (deploy-window compat): still calls showNotification with title/body', async () => {
    // Defensive legacy branch — kept for one deploy cycle so in-flight pushes
    // emitted by an old engine reaching a freshly-updated SW don't break.
    const { handlers, mockRegistration } = loadSw();
    const ev = pushEvent({
      title: 'Legacy',
      body: 'Old shape',
      notification_id: 'nid-legacy',
      thread_id: 'tid-1',
      tap: { kind: 'navigate', to: { target: 'thread', id: 'tid-1' } },
    });
    handlers.push(ev);
    await Promise.all(ev._waited);
    const [title, opts] = mockRegistration.showNotification.mock.calls[0];
    expect(title).toBe('Legacy');
    expect(opts.body).toBe('Old shape');
    expect(opts.tag).toBe('nid-legacy');
    // Legacy shape has no engine-built navigate URL — SW falls back to origin
    // root rather than rebuilding URL params (URL building moved to engine).
    expect(opts.navigate).toBe('https://example.com/');
    expect(opts.data).toEqual({
      notification_id: 'nid-legacy',
      thread_id: 'tid-1',
      event_id: undefined,
      tap: { kind: 'navigate', to: { target: 'thread', id: 'tid-1' } },
    });
  });

  it('non-JSON payload: defaults to Lucidos title + text body', async () => {
    // Belt-and-braces: a push body that fails JSON.parse falls through to
    // event.data.text() body. Still satisfies userVisibleOnly.
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

// Layer 3 — wake-push contract. The engine schedules a duplicate push 3 s
// after every real push to a macOS-Chrome subscription, carrying `wake:
// true`. SW must call showNotification (Chrome's userVisibleOnly budget)
// but with renotify:false + silent:true so the OS doesn't re-pop sound /
// banner — the original notification is already on screen. See
// system-knowhow/notifications.md §4.5.
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
      title: 'Claude is asking',
      body: 'Pick one',
      notification_id: 'nid-stuck',
      thread_id: 'tid-stuck',
      tap: { kind: 'navigate', to: { target: 'thread', id: 'tid-stuck' } },
      wake: true,
    });
    handlers.push(ev);
    await Promise.all(ev._waited);
    expect(mockRegistration.showNotification).toHaveBeenCalledTimes(1);
  });

  it('wake:true push sets renotify:false and silent:true (no re-pop, no sound)', async () => {
    const { handlers, mockRegistration } = loadSw();
    const ev = pushEvent({
      title: 'T', body: 'B', notification_id: 'nid-stuck', wake: true,
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
    const ev = pushEvent({ title: 'T', body: 'B', notification_id: 'nid-real' });
    handlers.push(ev);
    await Promise.all(ev._waited);
    const [, opts] = mockRegistration.showNotification.mock.calls[0];
    expect(opts.renotify).toBe(true);
    expect(opts.silent).toBe(false);
  });
});

// notificationclick — the macOS-Chrome tap path (Safari handles the tap
// declaratively and never runs this handler). routeToDeepLink delivers the
// deep link to an already-open Lucidos tab via postMessage, NOT via a
// fragment-only client.navigate(). The fragment-navigate path is unreliable:
// Chrome doesn't fire `hashchange` for a fragment-only WindowClient.navigate(),
// and the page-side focus/visibilitychange resume safety net doesn't fire when
// the tab the user clicked back into was already the focused/visible tab (the
// "came back to the computer in the morning" report — SW focused the right tab
// and marked the notification read, but the page never dispatched the deep
// link, so no modal opened / no thread navigation). See
// system-knowhow/notifications.md §4.5.
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

    // Deterministic page-side delivery: the structured deep link reaches the
    // page's navigator.serviceWorker 'message' listener regardless of whether
    // a hashchange fires or the tab was already focused.
    expect(client.postMessage).toHaveBeenCalledWith({ type: 'lucidos:deep-link', target: threadData });
    // The tab is brought forward.
    expect(client.focus).toHaveBeenCalledTimes(1);
    // Regression lock: the warm path must NOT fragment-navigate the tab — that
    // "succeeds" yet routes nothing (no hashchange, resume listener idle when
    // the tab was already focused). This is the morning-tap bug.
    expect(client.navigate).not.toHaveBeenCalled();
    // No duplicate window is opened when a tab already exists.
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

  // Behind the workspace gateway several workspaces share one origin
  // (/personal/, /dev/, …). A push for /personal is delivered to the /personal
  // SW, but clients.matchAll({includeUncontrolled:true}) returns EVERY same-origin
  // tab — including an open /dev tab. routeToDeepLink must only ever focus +
  // postMessage a tab in ITS OWN scope, else the tap lands in the wrong
  // workspace whose store has no such thread and "goes nowhere" (the reported
  // bug). These lock the per-scope client selection.
  const SCOPE_PERSONAL = 'https://example.com/personal/';

  it('cross-workspace: a tab in a DIFFERENT scope is not focused/messaged — opens a new window in THIS scope', async () => {
    const { handlers, matchAll, mockFetch, openWindow } = loadSw({ scope: SCOPE_PERSONAL });
    mockFetch.mockResolvedValue(new Response('ok'));
    // Only an out-of-scope /dev tab is open.
    const devTab = topLevelClient('https://example.com/dev/');
    matchAll.mockResolvedValue([devTab]);

    const ev = makeClickEvent(threadData);
    handlers.notificationclick(ev);
    await Promise.all(ev._waited);

    // The wrong-workspace tab is left alone — no hijack.
    expect(devTab.postMessage).not.toHaveBeenCalled();
    expect(devTab.focus).not.toHaveBeenCalled();
    // Instead a window is opened at the engine-built deep link, resolved against
    // THIS SW's scope (/personal/).
    expect(openWindow).toHaveBeenCalledWith(
      'https://example.com/personal/#notification=nid-thread&thread=tid-1&event=evt-7',
    );
  });

  it('cross-workspace: posts the deep link only to the SAME-scope tab when both are open', async () => {
    const { handlers, matchAll, mockFetch, openWindow } = loadSw({ scope: SCOPE_PERSONAL });
    mockFetch.mockResolvedValue(new Response('ok'));
    const devTab = topLevelClient('https://example.com/dev/');
    const personalTab = topLevelClient('https://example.com/personal/');
    // /dev listed first so an unfiltered find() would (wrongly) pick it.
    matchAll.mockResolvedValue([devTab, personalTab]);

    const ev = makeClickEvent(threadData);
    handlers.notificationclick(ev);
    await Promise.all(ev._waited);

    expect(personalTab.postMessage).toHaveBeenCalledWith({ type: 'lucidos:deep-link', target: threadData });
    expect(personalTab.focus).toHaveBeenCalledTimes(1);
    expect(devTab.postMessage).not.toHaveBeenCalled();
    expect(openWindow).not.toHaveBeenCalled();
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


// ADR 0013: behind the workspace gateway the SW is registered at /ws/<id>/sw.js
// with scope /ws/<id>/, so every same-origin path it matches or builds must be
// resolved against that scope. These lock in the scope-relative behavior.
describe('Service Worker base-path awareness (gateway scope)', () => {
  const SCOPE = 'https://example.com/ws/work/';

  it('intercepts /ws/<id>/api/v1 GETs (matched scope-relative)', () => {
    const { handlers, mockFetch } = loadSw({ scope: SCOPE });
    mockFetch.mockResolvedValue(new Response('ok'));
    const ev = makeEvent('https://example.com/ws/work/api/v1/threads/list');
    handlers.fetch(ev);
    expect(ev.respondWith).toHaveBeenCalled();
  });

  it('does NOT intercept the scoped SSE stream', () => {
    const { handlers } = loadSw({ scope: SCOPE });
    const ev = makeEvent('https://example.com/ws/work/api/v1/events');
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
    // The scoped root resolves scope-relative to '/' → the SPA shell handler.
    // (The browser only dispatches in-scope requests to a scoped SW, so an
    // out-of-scope origin-root navigation never reaches this worker.)
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
