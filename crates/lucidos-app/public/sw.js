// Lucidos Service Worker — handles push notifications and PWA support

// Base path this SW is scoped to (ADR 0014): "/<slug>/" behind the workspace
// gateway, else "/". Each workspace registers its own SW under /<slug>/sw.js
// with scope /<slug>/, so caches, the navigation shell, and push are
// per-workspace. Every same-origin path the SW matches or builds is resolved
// against this scope. `self.registration.scope` is the absolute scope URL;
// fall back to "/" if it's unavailable (defensive — should always be set).
const SCOPE_PATH = (() => {
  try {
    return new URL(self.registration.scope).pathname;
  } catch {
    return '/';
  }
})();

/** Strip the scope prefix from an absolute pathname, yielding a scope-relative
 *  path (always starting with "/"). `/work/api/v1/x` → `/api/v1/x`; `/` → `/`. */
function scopeRelative(path) {
  return path.startsWith(SCOPE_PATH) ? '/' + path.slice(SCOPE_PATH.length) : path;
}

/** Best-effort breadcrumb to the engine's client-log (same channel the page
 *  uses via postClientLog). Diagnostic only: it answers the otherwise-invisible
 *  question "does the SW's push / notificationclick run on iOS?" — iOS handles a
 *  declarative push in the parent process and may bypass the SW entirely, so the
 *  ABSENCE of these lines on iOS (while they appear on Chrome) is itself the
 *  signal. Returns the fetch promise so callers can keep the SW alive via
 *  waitUntil. Never throws (telemetry must not break the handler). */
function swLog(message, data) {
  try {
    return fetch(`${self.location.origin}${SCOPE_PATH}api/v1/internal/client-log`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ category: 'sw', message, data: data || {} }),
      keepalive: true,
    }).catch(() => {});
  } catch {
    return Promise.resolve();
  }
}

// Activate new SW immediately and take control of all pages.
// This triggers Chrome's "site updated" toast when sw.js changes,
// but only during development — in production SW updates are rare.
self.addEventListener('install', (event) => {
  self.skipWaiting();
  // Warm the navigation shell into SHELL_CACHE so an OFFLINE first navigation
  // (engine unreachable) still has an index.html to fall back to. The shell is
  // served network-first now (see networkFirstShell), so this is the offline
  // safety net — NOT the hot path. Best-effort: a miss is fine, the network-first
  // handler caches the shell on the first successful navigation. Built mode only
  // — `cache: 'reload'` bypasses the HTTP cache so we store the CURRENT build's
  // shell, and SHELL_CACHE is build-id-keyed so activate() purges it on a bump.
  if (IS_BUILT) {
    event.waitUntil((async () => {
      try {
        const shellKey = new Request(self.location.origin + SCOPE_PATH);
        const resp = await fetch(self.location.origin + SCOPE_PATH, { cache: 'reload' });
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
  // Scope-relative path so the same matching works at the root AND under
  // /<slug>/ behind the gateway. Cache keys still use the full event.request
  // URL (with the /<slug>/ prefix), so workspaces stay cache-isolated.
  const rel = scopeRelative(path);

  if (rel.startsWith('/api/v1/')) {
    // SSE: Chrome keeps the SW alive for the whole streaming connection, so
    // intercepting it hangs the worker — let the browser handle it natively.
    if (rel === '/api/v1/events') return;
    // Content-addressed blobs are immutable for the lifetime of the hash.
    if (rel.startsWith('/api/v1/blobs/')) {
      event.respondWith(cacheFirst(event.request, BLOB_CACHE));
      return;
    }
    // Other GETs get the iOS empty-response retry workaround (see top comment).
    event.respondWith(fetchWithRetry(event.request));
    return;
  }

  // Content-hashed app bundles — no-op in dev (Vite never serves /assets/*).
  // Refuse to cache an HTML response here (see isHtmlResponse): a bundle deleted
  // by a later build resolves through the server's SPA fallback to index.html,
  // and caching that under the bundle URL poisons the entry permanently.
  if (rel.startsWith('/assets/')) {
    event.respondWith(
      cacheFirst(event.request, SHELL_CACHE, (r) => !!r && r.ok && !isHtmlResponse(r)),
    );
    return;
  }

  // Navigation shell (index.html). Every top-level navigation lands here: the
  // PWA start URL and — the case that matters — every notification-tap reload,
  // which arrives as a cross-document `/?notification=…&thread=…` load (the
  // WebKit constraint in system-knowhow/notifications.md §4.5). Served
  // network-first (see networkFirstShell) so the shell always matches the
  // server's current /assets/* bundles; the cache is the offline fallback only.
  // The `path === '/'` gate is load-bearing: app-UI iframes (`/app/<id>/`) and
  // skill UIs are ALSO `mode: 'navigate'` requests but are NOT the SPA shell —
  // they must reach their own server-rendered HTML, never index.html. Built mode
  // only (IS_BUILT) so the dev server keeps serving a network-fresh shell.
  if (IS_BUILT && event.request.mode === 'navigate' && rel === '/') {
    event.respondWith(networkFirstShell(event.request));
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

// True if a response is an HTML document. A content-hashed /assets/* URL must
// resolve to its JS/CSS bundle or to nothing — but the dev server's SPA fallback
// answers a bundle deleted by a later `vite build --watch` rebuild with
// index.html (200 text/html), not a 404. Caching that HTML under the bundle's
// URL poisons the entry forever: the page then loads an HTML document as a module
// script, no JS runs, and the PWA paints a black #app. So the asset branch
// refuses to cache an HTML response — it's a stale-bundle miss, not the asset.
function isHtmlResponse(response) {
  const ct = response && response.headers && response.headers.get('content-type');
  return typeof ct === 'string' && ct.includes('text/html');
}

// Cache-first for immutable content (content-addressed blobs, content-hashed app
// bundles): serve from the Cache API on a hit with no network, otherwise fetch
// (with the iOS SW-restart retry) and populate the cache off the response path.
// `isCacheable(response)` decides what's worth storing — defaults to "any 2xx"
// so a transient 404/5xx during a deploy swap is never pinned; the asset branch
// passes a stricter predicate that also rejects an HTML SPA-fallback body.
async function cacheFirst(request, cacheName, isCacheable) {
  const cache = await caches.open(cacheName);
  const cached = await cache.match(request);
  if (cached) return cached;
  const response = await fetchWithRetry(request);
  const cacheable = isCacheable ? isCacheable(response) : (response && response.ok);
  if (cacheable) {
    // Clone off the response path so delivery isn't blocked on the cache write.
    cache.put(request, response.clone()).catch(() => {});
  }
  return response;
}

// Network-FIRST for the navigation shell (index.html). The shell must stay in
// lockstep with the content-hashed /assets/* bundles the server CURRENTLY has:
// every `vite build --watch` rebuild emits new bundle hashes and deletes the old
// ones, so a shell cached from an earlier build references bundles that are gone.
// The server's SPA fallback then answers those deleted bundles with index.html
// (200 text/html), the page loads HTML as its entry module script, no JS runs,
// and the PWA is a black #app — and with nothing loaded, nothing can self-heal.
// (That is why a long-lived iOS PWA went black while a frequently-reloaded
// desktop tab stayed fine: same SW, but the desktop tab kept fetching fresh
// shells.) Fetching the shell fresh when online keeps it matched to the server's
// assets; the cache is only the OFFLINE fallback. The heavy JS/CSS graph stays
// cache-first (immutable by hash), so only the ~9 KB HTML round-trips — and on a
// notification tap the engine just pushed, so it's reachable and that fetch is
// fast. Keyed by a NORMALIZED `/` request (no query string) so every
// `/?notification=…` deep-link variant shares one cached entry. A redirected or
// non-2xx response is never cached (a redirected Response replayed for a
// navigation throws; a 502 mid engine-restart must not be pinned). A non-ok
// response falls back to the last good cached shell — EXCEPT the gateway's boot
// splash (503 marked `X-Lucidos-Boot-Splash`), which is shown as-is so the user
// sees the "workspace starting…" page instead of a stale shell that 503-storms.
async function networkFirstShell(request) {
  const cache = await caches.open(SHELL_CACHE);
  const shellKey = new Request(self.location.origin + SCOPE_PATH);
  let response;
  try {
    response = await fetchWithRetry(request);
  } catch (err) {
    // Offline / engine down — serve the last good shell if install precached one.
    const cached = await cache.match(shellKey);
    if (cached) return cached;
    throw err;
  }
  if (response && response.ok && !response.redirected) {
    cache.put(shellKey, response.clone()).catch(() => {});
    return response;
  }
  // The gateway answers a navigation to a stopped / cold-booting workspace with a
  // 503 (ADR 0014 §11): the branded boot SPLASH (Retry-After + meta-refresh) for
  // a document navigation it lazy-starts, or a plain "workspace stopped" body.
  // Either way a 503 means the engine is NOT serving — so the cached app shell is
  // exactly the wrong thing to show: it boots and 503-storms its API calls
  // against the down engine, surfacing as a red connection dot (assets cached) or
  // a white screen (assets evicted / different build). The splash, by contrast,
  // is a real page the user SHOULD see — its meta-refresh transitions to the app
  // once the engine is up. So a 503 is shown AS-IS (never cached — it's a 503),
  // independent of the `X-Lucidos-Boot-Splash` marker: the gateway is a
  // machine-global daemon that does NOT restart on a CC Apply, so a freshly-built
  // gateway carrying the marker may not be the one actually running — keying on
  // the 503 status keeps the splash working against an older gateway too.
  if (response && (response.status === 503 || response.headers.get('x-lucidos-boot-splash'))) {
    return response;
  }
  // Other non-ok (e.g. a 502 mid engine-restart, where the engine is up but a
  // proxied request transiently failed) — prefer a cached good shell over the
  // error page.
  const cached = await cache.match(shellKey);
  return cached || response;
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
    return self.location.origin + SCOPE_PATH;
  }
  // The engine builds SCOPE-RELATIVE deep links (`#thread=…`, `?notification=…`)
  // — it doesn't know its `/<slug>` prefix, so it leaves the path off entirely
  // and lets resolution against the subscription scope supply it. Resolve them
  // against the SW's scope so the tap lands in THIS workspace: defensively drop
  // any leading slash (legacy in-flight pushes still ship `/#…` / `/?…`, which
  // would escape to the origin root), then resolve against `origin + SCOPE_PATH`
  // (trailing slash).
  try {
    const scopeRelativeUrl = relativeUrl.replace(/^\//, '');
    return new URL(scopeRelativeUrl, self.location.origin + SCOPE_PATH).toString();
  } catch {
    return self.location.origin + SCOPE_PATH;
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

  // App-icon badge (Badging API). The engine carries the workspace's current
  // unread count in the TOP-LEVEL `app_badge` field (sibling of `web_push` /
  // `notification`, see build_push_payload). Mirror it onto the installed PWA
  // icon so a CLOSED workspace PWA stays accurate on Chrome/Android — a
  // per-workspace PWA badges ITS OWN workspace's unreads. iOS sets the badge
  // natively from the same field without running this handler, so this is the
  // non-iOS path. Best-effort + feature-gated; `0` clears it.
  const appBadge =
    raw && typeof raw.app_badge === 'number' ? raw.app_badge : null;

  // Every push must leave a visible notification on screen when waitUntil
  // resolves — the userVisibleOnly:true contract. This applies to iOS too: an
  // experiment that skipped showNotification on iOS+declarative (betting the OS
  // would render the declarative notification as a fallback) showed NOTHING —
  // iOS only uses the declarative fallback when the SW push handler ERRORS/times
  // out, not when it cleanly resolves without displaying. So we ALWAYS display
  // here, on every platform. (The iOS running+icon-launched deep-link bug — tap
  // focuses without navigating — is an unfixed WebKit limitation upstream of
  // this handler; see system-knowhow/notifications.md §4.5, seventeenth
  // iteration. Showing the notification but maybe-not-navigating is strictly
  // better than showing nothing.)
  // Diagnostic breadcrumb (kept): records that the SW push handler ran — on iOS
  // it does fire, contrary to the old "iOS bypasses the SW entirely" claim.
  swLog('push', {
    declarative: isDeclarative,
    wake: isWake,
    has_navigate: !!navigateRelative,
    notification_id: (data && data.notification_id) || null,
  });
  event.waitUntil(self.registration.showNotification(title, {
    body,
    icon: SCOPE_PATH + 'favicon.svg',
    badge: SCOPE_PATH + 'favicon.svg',
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
    // at all — the OS already handled the push declaratively.
    navigate: navigateRelative
      ? resolveNavigate(navigateRelative)
      : self.location.origin + SCOPE_PATH,
    data,
  }));

  if (appBadge !== null && typeof self.navigator.setAppBadge === 'function') {
    // clearAppBadge ships paired with setAppBadge, but guard it anyway so a 0
    // count can't throw synchronously (before .catch) on a partial impl —
    // matches the page-side applyAppBadge's optional-chaining.
    const badgePromise =
      appBadge > 0 ? self.navigator.setAppBadge(appBadge) : self.navigator.clearAppBadge?.();
    if (badgePromise) event.waitUntil(badgePromise.catch(() => {}));
  }
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
    : self.location.origin + SCOPE_PATH;

  // Mark-read runs in parallel with navigation; Promise.all's final await
  // keeps the SW alive until both finish. Without that await, iOS terminates
  // the SW the moment navigation resolves and cancels the in-flight POST
  // (unread badge stays stuck).
  const markReadPromise = notificationId
    ? fetch(`${self.location.origin}${SCOPE_PATH}api/v1/notification/read?id=${encodeURIComponent(notificationId)}`, {
        method: 'POST',
      }).catch(() => {})
    : Promise.resolve();

  // Diagnostic: record that the SW notificationclick fired. The decisive iOS
  // question — if this line appears for an iPhone UA, iOS DOES run the SW on tap
  // and we can deliver the deep link via postMessage (the reliable Chrome path)
  // instead of the flaky declarative navigate; if it never appears on iOS (while
  // it does on Chrome), iOS is purely declarative and bypasses the SW.
  const logPromise = swLog('notificationclick', {
    notification_id: notificationId,
    has_navigate: !!data.navigate,
    target_url: targetUrl.slice(0, 200),
  });

  event.waitUntil(Promise.all([logPromise, markReadPromise, routeToDeepLink(targetUrl, data)]));
});

// True if a top-level Window client belongs to THIS service worker's scope.
// Behind the workspace gateway several workspaces share one origin
// (`/personal/`, `/dev/`, …); a push for `/personal` is delivered to the
// `/personal` SW, but `clients.matchAll({includeUncontrolled:true})` returns
// EVERY same-origin tab — including an open `/dev` tab. Without this gate
// routeToDeepLink would focus + postMessage the wrong workspace's tab, whose
// store has no such thread/app, so the tap "goes nowhere" (the cross-workspace
// notification-tap bug). Match only same-scope clients; tolerate a missing
// trailing slash (`/personal` for scope `/personal/`). At the legacy root scope
// (`/`) every same-origin client matches — correct, since there's only one
// workspace there.
function clientInScope(clientUrl) {
  let pathname;
  try {
    pathname = new URL(clientUrl).pathname;
  } catch {
    return false;
  }
  // startsWith covers the exact-match case (SCOPE_PATH ends with '/'); the
  // second clause adds tolerance for a client at the slug with no trailing slash.
  return pathname.startsWith(SCOPE_PATH) || pathname + '/' === SCOPE_PATH;
}

async function routeToDeepLink(targetUrl, tapData) {
  const windowClients = await clients.matchAll({ type: 'window', includeUncontrolled: true });
  // frameType filter is the load-bearing line — same-origin app-UI iframes
  // (src=/app/<id>/) are ALSO Window clients per the SW spec. Without the
  // 'top-level' gate, find() can return the iframe; focusing/messaging it
  // moves the wrong surface, manifesting as "PWA opens to the wrong view".
  // clientInScope keeps the tap inside THIS workspace (gateway multi-workspace).
  // URL-substring filters stay as belt-and-braces for non-iframe edge cases
  // (skill UIs in their own window).
  const appClient = windowClients.find(c =>
    c.frameType === 'top-level' &&
    clientInScope(c.url) &&
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
