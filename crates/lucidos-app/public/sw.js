// Lucidos Service Worker — handles push notifications and PWA support

// Activate new SW immediately and take control of all pages.
// This triggers Chrome's "site updated" toast when sw.js changes,
// but only during development — in production SW updates are rare.
self.addEventListener('install', (event) => {
  self.skipWaiting();
  // Warm the navigation shell so the FIRST controlled reload paints from disk
  // instead of paying an HTML round trip. The iOS notification-tap is always a
  // full cross-document reload (system-knowhow/notifications.md §4.5), so on a
  // slow link that one uncached HTML fetch is visible blank time. Best-effort:
  // if this misses (engine down at install), the fetch handler's cache-first
  // populates the shell on the first navigation instead. Built mode only —
  // `cache: 'reload'` bypasses the HTTP cache so we store the CURRENT build's
  // shell, and SHELL_CACHE is build-id-keyed so activate() purges it on a bump.
  if (IS_BUILT) {
    event.waitUntil((async () => {
      try {
        const shellKey = new Request(self.location.origin + '/');
        const resp = await fetch(self.location.origin + '/', { cache: 'reload' });
        if (resp && resp.ok && !resp.redirected) {
          const cache = await caches.open(SHELL_CACHE);
          await cache.put(shellKey, resp.clone());
        }
      } catch {
        /* offline / engine down at install — first navigation populates it */
      }
    })());
  }
});
self.addEventListener('activate', (event) => {
  // Take control of open pages, then drop any cache we no longer recognize so
  // a bumped cache name (e.g. lucidos-shell-v1 -> -v2) purges the prior
  // generation instead of leaking it forever.
  event.waitUntil(
    (async () => {
      const names = await caches.keys();
      await Promise.all(
        names.filter((name) => !KEEP_CACHES.includes(name)).map((name) => caches.delete(name)),
      );
      await clients.claim();
    })(),
  );
});

// Required for iOS PWA — without a fetch listener iOS won't treat this as a valid SW.
//
// We intercept same-origin GET /api/v1/* requests with explicit respondWith(fetch())
// to work around an iOS Safari bug where the implicit "don't call respondWith"
// fallback returns empty/corrupted responses after the SW is killed and restarted
// under memory pressure (manifests as blank thread views). Built /assets/*
// bundles are also served Cache-first (see SHELL_CACHE) to speed up reloads.
//
// We DO NOT intercept non-GET methods (POST/PUT/PATCH/DELETE). iOS WebKit's body
// stream cloning is unreliable when respondWith re-issues a request with a body —
// large bodies (e.g. base64-encoded image uploads) reject with "TypeError: Load
// failed", surfacing as "Failed to send message" toasts even though the request
// would succeed if the browser handled it natively.
//
// We also exclude SSE /api/v1/events because Chrome keeps the SW alive for the entire
// streaming connection, causing hangs when two SW versions coexist (active + waiting).
//
// For GETs we wrap fetch in a single retry — iOS occasionally rejects the first
// request when the SW just woke from suspension, and a retry succeeds.
//
// Content-addressed blob endpoints (/api/v1/blobs/<hash>[/preview]) get an extra
// Cache API layer on top — the bytes for a hash never change, so a Cache entry
// is valid forever and survives iOS PWA HTTP-cache eviction. Without this, every
// thread visit re-fetches the preview and the empty <img> shows the dark page
// background through it during the round-trip (the visible "black flash").
const BLOB_CACHE = 'lucidos-blob-v1';

// Per-build id, stamped into the built sw.js by the `lucidos-sw-stamp` Vite
// plugin (vite.config.ts). It both keys the shell cache (so a new build's SW
// purges the prior build's cache in activate()) AND makes each build's sw.js
// byte-different — that byte difference is what the browser's SW update check
// detects, which fires the "New version available → Refresh" toast in built
// mode (the user's signal that a rebuild is ready to reload). In the live dev
// server the plugin doesn't run, so this stays the literal placeholder — a
// harmless static name (the dev server never serves /assets/*, so the shell
// cache never populates anyway).
const BUILD_ID = '__LUCIDOS_BUILD_ID__';

// Immutable, content-hashed production bundles. Vite builds the app to
// /assets/<name>-<hash>.<ext>; the hash changes whenever the bytes change, so a
// cached entry for a URL is valid forever. Caching these lets a reload pull the
// whole JS/CSS graph from disk instead of the network — the bulk of the cost of
// an iOS PWA reload after a notification-tap navigation (which is always a full
// cross-document load; see system-knowhow/notifications.md §4.5). Self-gating
// across run modes: the Vite dev server serves source modules from /src,
// /@vite, /@id, /node_modules/.vite — never /assets/* — so this never fires in
// dev (where caching unhashed modules would pin stale code and break HMR) and
// engages only for a built deployment.
const SHELL_CACHE = 'lucidos-shell-' + BUILD_ID;

// True only in a built deployment. The `lucidos-sw-stamp` plugin rewrites every
// `__LUCIDOS_BUILD_ID__` token to a 12-char hex id at `vite build` time, so an
// un-stamped sw.js (the live dev server, and the Vitest source-string tests)
// still starts with `__`; a real build id never does. Gates the navigation
// shell cache below to built mode only — in the Vite dev server the shell
// (index.html → /src/main.tsx) and its module graph must stay network-fresh or
// an edit to index.html / HMR would be pinned to a stale copy. The `/assets/*`
// branch self-gates by path instead (dev never serves /assets/*), but a
// navigation to `/` happens in BOTH modes, so it needs this explicit gate.
const IS_BUILT = !BUILD_ID.startsWith('__');

// Caches the SW owns and keeps; activate() deletes anything else so bumping a
// cache name purges the prior generation.
const KEEP_CACHES = [BLOB_CACHE, SHELL_CACHE];

self.addEventListener('fetch', (event) => {
  if (event.request.method !== 'GET') return;
  const url = event.request.url;
  // Only ever touch our own origin — cross-origin requests (CDNs, third-party
  // iframes) are none of the SW's business.
  if (!url.startsWith(self.location.origin)) return;
  const path = url.slice(self.location.origin.length).split('?')[0];

  if (path.startsWith('/api/v1/')) {
    // SSE: Chrome keeps the SW alive for the whole streaming connection, so
    // intercepting it hangs the worker — let the browser handle it natively.
    if (path === '/api/v1/events') return;
    // Content-addressed blobs are immutable for the lifetime of the hash.
    if (path.startsWith('/api/v1/blobs/')) {
      event.respondWith(cacheFirst(event.request, BLOB_CACHE));
      return;
    }
    // Other GETs get the iOS empty-response retry workaround (see top comment).
    event.respondWith(fetchWithRetry(event.request));
    return;
  }

  // Content-hashed app bundles — no-op in dev (Vite never serves /assets/*).
  if (path.startsWith('/assets/')) {
    event.respondWith(cacheFirst(event.request, SHELL_CACHE));
    return;
  }

  // Navigation shell (index.html). Every top-level navigation lands here: the
  // PWA start URL and — the case that matters — every notification-tap reload,
  // which arrives as a cross-document `/?notification=…&thread=…` load (the
  // WebKit constraint in system-knowhow/notifications.md §4.5). Serving the
  // cached shell turns that reload's HTML fetch into a disk read; combined with
  // the cached /assets/* graph above, the app boots with zero network on the
  // critical path and only the data GETs still round-trip. The `path === '/'`
  // gate is load-bearing: app-UI iframes (`/app/<id>/`) and skill UIs are ALSO
  // `mode: 'navigate'` requests but are NOT the SPA shell — they must reach
  // their own server-rendered HTML, never index.html. Built mode only
  // (IS_BUILT) so the dev server keeps serving a network-fresh shell.
  if (IS_BUILT && event.request.mode === 'navigate' && path === '/') {
    event.respondWith(cacheFirstShell(event.request));
    return;
  }
});

async function fetchWithRetry(request) {
  try {
    return await fetch(request);
  } catch {
    return await fetch(request);
  }
}

// Cache-first for immutable content (content-addressed blobs, content-hashed app
// bundles): serve from the Cache API on a hit with no network, otherwise fetch
// (with the iOS SW-restart retry) and populate the cache off the response path.
// Only successful responses are cached, so a transient 404/5xx during a deploy
// swap is never pinned.
async function cacheFirst(request, cacheName) {
  const cache = await caches.open(cacheName);
  const cached = await cache.match(request);
  if (cached) return cached;
  const response = await fetchWithRetry(request);
  if (response && response.ok) {
    // Clone off the response path so delivery isn't blocked on the cache write.
    cache.put(request, response.clone()).catch(() => {});
  }
  return response;
}

// Cache-first for the navigation shell (index.html). Unlike cacheFirst, the
// entry is keyed by a NORMALIZED `/` request (no query string) so every
// `/?notification=…` deep-link variant collapses onto one shell entry — read
// and write both use `shellKey`, never the query-bearing `request`. Lives in
// SHELL_CACHE so a new build's activate() purges it alongside the stale
// /assets graph. A redirected or non-2xx response is never cached: a redirected
// Response replayed for a navigation throws ("response served by service worker
// has redirections"), and a transient error page during an engine restart must
// not be pinned. On a miss the live network response is returned as-is.
async function cacheFirstShell(request) {
  const cache = await caches.open(SHELL_CACHE);
  const shellKey = new Request(self.location.origin + '/');
  const cached = await cache.match(shellKey);
  if (cached) return cached;
  const response = await fetchWithRetry(request);
  if (response && response.ok && !response.redirected) {
    cache.put(shellKey, response.clone()).catch(() => {});
  }
  return response;
}

// Notification tag — used so a repeat push for the same notification_id
// replaces the OS-level notification instead of stacking duplicates.
const DEFAULT_NOTIFICATION_TAG = 'lucidos-notification';

// Resolve the engine-built relative navigate URL against the SW's origin so
// `clients.openWindow()` (which requires an absolute URL in Chrome) gets a
// fully-qualified URL. The push handler also resolves it for the
// `showNotification` `navigate` option. Safari handles the declarative
// `navigate` field directly without running this SW, so it doesn't go here.
function resolveNavigate(relativeUrl) {
  if (typeof relativeUrl !== 'string' || relativeUrl.length === 0) {
    return self.location.origin + '/';
  }
  // Hash-only path starts with `/#` or `/`. URL constructor handles both.
  try {
    return new URL(relativeUrl, self.location.origin).toString();
  } catch {
    return self.location.origin + '/';
  }
}

self.addEventListener('push', (event) => {
  let raw = null;
  try {
    raw = event.data.json();
  } catch {
    // Non-JSON or empty payload — fall back to a generic notification so we
    // still satisfy `userVisibleOnly: true` and don't burn the silent-push
    // budget. Bare-text payloads land in `body`. Stamped at the top level
    // (legacy shape) so the legacy-branch readers below pick them up.
    raw = { title: 'Lucidos', body: event.data?.text() || 'New notification' };
  }

  // Two payload shapes — the engine ships Declarative Web Push
  // (`{web_push: 8030, notification: {…}}`, see
  // crates/lucidos-engine/src/scheduler/push.rs::build_push_payload), but a
  // sub-population of in-flight pushes during a deploy window may arrive in
  // the legacy flat shape. The legacy branch is intentionally narrow — kept
  // for one cycle, then removable once monitoring confirms no flat-shape
  // pushes are still being sent.
  const isDeclarative =
    raw && typeof raw === 'object' && raw.web_push === 8030
    && raw.notification && typeof raw.notification === 'object';

  // `wake: true` rides at the TOP LEVEL (sibling to `web_push` / `notification`)
  // so Safari ignores it. Layer 3 of the macOS-Chrome partial-wedge
  // mitigation (system-knowhow/notifications.md §4.5): every 3 s after a
  // real push to a macOS-Chrome device the engine sends a wake push with
  // identical content + `wake: true`; the SW gates `renotify` / `silent`
  // off it so the user sees no visible re-pop.
  const isWake = raw && raw.wake === true;

  const title = isDeclarative
    ? (raw.notification.title || 'Lucidos')
    : (raw && raw.title) || 'Lucidos';
  // Both branches default to 'New notification' on an empty body so the user
  // never sees a title-only notification with a blank subtitle line.
  const body = isDeclarative
    ? (raw.notification.body || 'New notification')
    : (raw && raw.body) || 'New notification';
  const tag = isDeclarative
    ? (raw.notification.tag || DEFAULT_NOTIFICATION_TAG)
    : ((raw && raw.notification_id) || DEFAULT_NOTIFICATION_TAG);
  const navigateRelative = isDeclarative
    ? raw.notification.navigate
    : null;

  // `data` is what `event.notification.data` returns inside notificationclick.
  // Declarative: read straight from `notification.data` (engine duplicates the
  // navigate URL in there for the click handler). Legacy: rebuild from flat
  // top-level fields. Both paths carry the structured `tap`.
  const data = isDeclarative
    ? (raw.notification.data || {})
    : {
        notification_id: raw && raw.notification_id,
        thread_id: raw && raw.thread_id,
        event_id: raw && raw.event_id,
        tap: raw && raw.tap,
      };

  // Every push must leave a visible notification on screen when waitUntil
  // resolves — that's the userVisibleOnly:true contract Chrome enforces on
  // the subscription. Skipping showNotification counts as a "silent push"
  // against Chrome's per-origin budget; once exhausted Chrome either injects
  // its generic "site updated in background" notification or stops
  // delivering pushes entirely.
  event.waitUntil(self.registration.showNotification(title, {
    body,
    icon: '/favicon.svg',
    badge: '/favicon.svg',
    tag,
    renotify: !isWake,
    requireInteraction: true,
    silent: isWake,
    // `navigate` is the spec-level bypass for the macOS-Chrome partial-wedge
    // bug (Safari 18.5+ honors it; Chrome ignores it until #382298314 ships
    // and falls through to notificationclick). This is the engine's QUERY
    // (cross-document) navigate URL — the form iOS needs to navigate an
    // already-open window; the warm Chrome notificationclick path above uses
    // the HASH `data.navigate` instead. Combined with
    // `launch_handler: { client_mode: "navigate-existing" }` in manifest.json,
    // the existing PWA window is reused. For Safari this path doesn't run
    // at all — the OS already handled the push declaratively. See
    // system-knowhow notifications.md §4.5.
    navigate: navigateRelative
      ? resolveNavigate(navigateRelative)
      : self.location.origin + '/',
    data,
  }));
});

self.addEventListener('message', (event) => {
  // Liveness probe: the page periodically pings to verify Chrome can still
  // wake this SW. If the SW is wedged (Chrome can't deliver events to it —
  // the symptom is notification clicks doing nothing even though `close()`
  // is the first statement of notificationclick), no pong reaches the page
  // and the page recovers via unregister + re-register.
  if (event.data?.type === 'lucidos:ping' && event.source) {
    event.source.postMessage({ type: 'lucidos:pong' });
    return;
  }
  // Build-id query (control panel debugging aid): report which build this SW
  // is. BUILD_ID is the ground truth for "did the new build's SW take over?" —
  // it's the same value whose byte-change makes sw.js differ and fires the
  // "New version available → Refresh" toast. The page surfaces it in the
  // connection status popover so a missed toast can be diagnosed by eye.
  if (event.data?.type === 'lucidos:get-build-id' && event.source) {
    event.source.postMessage({ type: 'lucidos:build-id', buildId: BUILD_ID });
  }
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();

  const data = event.notification.data || {};
  const notificationId = data.notification_id || null;

  // This handler runs on Chrome (and any browser that doesn't natively process
  // the declarative envelope). iOS Safari handles the tap declaratively and
  // navigates to `notification.navigate` (the QUERY form) WITHOUT running this
  // handler. See crates/lucidos-engine/src/scheduler/push.rs::build_push_payload.
  //
  // A warm, already-open tab is routed by postMessage (see routeToDeepLink) —
  // NOT by this URL — so `targetUrl` only drives the cold `clients.openWindow`
  // path when no Lucidos tab is open. It uses the HASH form of the deep link
  // from `data.navigate` so the freshly-opened page's `handleHashLocation`
  // cold-start router reads the params off the hash.
  //
  // The legacy SW-side `buildDeepLinkUrl` was deleted with the declarative
  // migration — a missing `data.navigate` therefore means the push arrived in
  // legacy flat shape (in-flight during deploy) AND with no usable deep-link
  // payload; fall back to opening the app at root.
  const targetUrl = data.navigate
    ? resolveNavigate(data.navigate)
    : self.location.origin + '/';

  // Mark-read runs in parallel with navigation; Promise.all's final await
  // keeps the SW alive until both finish. Without that await, iOS terminates
  // the SW the moment navigation resolves and cancels the in-flight POST
  // (unread badge stays stuck).
  const markReadPromise = notificationId
    ? fetch(`${self.location.origin}/api/v1/notification/read?id=${encodeURIComponent(notificationId)}`, {
        method: 'POST',
      }).catch(() => {})
    : Promise.resolve();

  event.waitUntil(Promise.all([markReadPromise, routeToDeepLink(targetUrl, data)]));
});

async function routeToDeepLink(targetUrl, tapData) {
  const windowClients = await clients.matchAll({ type: 'window', includeUncontrolled: true });
  // frameType filter is the load-bearing line — same-origin app-UI iframes
  // (src=/app/<id>/) are ALSO Window clients per the SW spec. Without the
  // 'top-level' gate, find() can return the iframe; focusing/messaging it
  // moves the wrong surface, manifesting as "PWA opens to the wrong view".
  // URL-substring filters stay as belt-and-braces for non-iframe edge cases
  // (skill UIs in their own window).
  const appClient = windowClients.find(c =>
    c.frameType === 'top-level' &&
    !c.url.includes('/sw.js') &&
    !c.url.includes('/api/v1/skill/')
  );

  if (appClient) {
    // Bring the tab forward (focus() also unfreezes a Chrome-frozen page so it
    // can process the message below), then hand the page the structured deep
    // link. postMessage → the page's navigator.serviceWorker 'message' listener
    // (onServiceWorkerMessage in useStartup) → dispatchDeepLink, the SAME
    // router the URL path uses.
    //
    // We deliberately do NOT use a fragment-only client.navigate('/#…') here.
    // It "succeeds" against the warm, SW-controlled tab (so an earlier
    // navigate-first design returned without ever messaging the page) yet
    // routes nothing: Chrome does not fire `hashchange` for a fragment-only
    // WindowClient.navigate(), and the page-side focus/visibilitychange resume
    // safety net does NOT fire when the tab the user clicked back into was
    // already the focused/visible tab — the "came back to the computer in the
    // morning" case, where the SW focused the right tab and marked the
    // notification read but the deep link silently no-op'd. postMessage is
    // independent of both. See system-knowhow/notifications.md §4.5.
    await appClient.focus().catch(() => {});
    appClient.postMessage({ type: 'lucidos:deep-link', target: tapData });
    return;
  }
  // No existing top-level Lucidos window — open one at the engine-built
  // deep-link URL so cold-start routing (handleHashLocation on load) picks up
  // the params.
  await clients.openWindow(targetUrl);
}
