// Lucidos Service Worker — handles push notifications and PWA support

// Activate new SW immediately and take control of all pages.
// This triggers Chrome's "site updated" toast when sw.js changes,
// but only during development — in production SW updates are rare.
self.addEventListener('install', () => {
  self.skipWaiting();
});
self.addEventListener('activate', (event) => {
  event.waitUntil(clients.claim());
});

// Required for iOS PWA — without a fetch listener iOS won't treat this as a valid SW.
//
// We intercept same-origin GET /api/v1/* requests with explicit respondWith(fetch())
// to work around an iOS Safari bug where the implicit "don't call respondWith"
// fallback returns empty/corrupted responses after the SW is killed and restarted
// under memory pressure (manifests as blank thread views).
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

self.addEventListener('fetch', (event) => {
  const url = event.request.url;
  if (!url.startsWith(self.location.origin + '/api/v1/')) return;
  if (event.request.method !== 'GET') return;
  const path = url.slice(self.location.origin.length).split('?')[0];
  if (path === '/api/v1/events') return;
  if (path.startsWith('/api/v1/blobs/')) {
    event.respondWith(fetchBlobWithCache(event.request));
    return;
  }
  event.respondWith(fetchWithRetry(event.request));
});

async function fetchWithRetry(request) {
  try {
    return await fetch(request);
  } catch {
    return await fetch(request);
  }
}

async function fetchBlobWithCache(request) {
  const cache = await caches.open(BLOB_CACHE);
  const cached = await cache.match(request);
  if (cached) return cached;
  const response = await fetchWithRetry(request);
  if (response.ok) {
    // Clone off the response path so delivery isn't blocked on the cache write.
    cache.put(request, response.clone()).catch(() => {});
  }
  return response;
}

// Notification tag — used so a repeat push for the same notification_id
// replaces the OS-level notification instead of stacking duplicates.
const DEFAULT_NOTIFICATION_TAG = 'lucidos-notification';

// Resolve the engine-built relative navigate URL against the SW's origin so
// `client.navigate()` and `clients.openWindow()` (both of which require
// absolute URLs in Chrome) get a fully-qualified URL. Safari handles the
// declarative `navigate` field directly without running this SW, so it
// doesn't go through here.
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
  }
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();

  const data = event.notification.data || {};
  const notificationId = data.notification_id || null;

  // This handler runs on Chrome (and any browser that doesn't natively process
  // the declarative envelope). It reads the HASH form of the deep link from
  // `data.navigate`. iOS Safari handles the tap declaratively and navigates to
  // `notification.navigate` (the QUERY form) WITHOUT running this handler — the
  // two forms exist because iOS won't apply a same-document (hash-only)
  // navigation to an already-open PWA window, so iOS needs a cross-document
  // (query) URL. See crates/lucidos-engine/src/scheduler/push.rs::build_push_payload.
  //
  // Hash here (not query) keeps a warm Chrome tap reload-free: navigating a
  // controlled tab from `/` to `/#…` is a same-document change, so the page is
  // NOT reloaded and its hashchange listener routes the deep link.
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
  // 'top-level' gate, find() can return the iframe; navigate() then changes
  // the iframe's URL and the PWA main window doesn't move, manifesting as
  // "PWA opens to the wrong view". URL-substring filters stay as belt-and-
  // braces for non-iframe edge cases (skill UIs in their own window).
  const appClient = windowClients.find(c =>
    c.frameType === 'top-level' &&
    !c.url.includes('/sw.js') &&
    !c.url.includes('/api/v1/skill/')
  );

  if (appClient) {
    // 1. Try navigate first — works for both desktop Chrome when this SW
    //    controls the tab AND iOS PWA cold-start (the inert WindowClient
    //    accepts navigate even when focus/postMessage silently drop).
    //    navigate() rejects with TypeError when the target tab is NOT
    //    controlled by this SW (dev hard-reload, DevTools "Update on
    //    reload" dropping the controller). matchAll({includeUncontrolled:true})
    //    still returns the tab, so we fall through to focus() below.
    const navigated = await appClient.navigate(targetUrl).catch(() => null);
    if (navigated) {
      await navigated.focus().catch(() => {});
      return;
    }
    // 2. focus() works on uncontrolled clients; the page-side message
    //    listener in useStartup dispatches the deep link through the same
    //    router the hashchange path uses. Skip the openWindow fallback
    //    when this succeeds so the user's existing tab is reused.
    try {
      await appClient.focus();
      appClient.postMessage({ type: 'lucidos:deep-link', target: tapData });
      return;
    } catch { /* fall through to openWindow */ }
  }
  await clients.openWindow(targetUrl);
}
