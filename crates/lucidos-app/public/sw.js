// Lucidos Service Worker — handles push notifications and PWA support

// Activate new SW immediately and take control of all pages.
// This triggers Chrome's "site updated" toast when sw.js changes,
// but only during development — in production SW updates are rare.
self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (event) => event.waitUntil(clients.claim()));

// Required for iOS PWA — without a fetch listener iOS won't treat this as a valid SW.
//
// We intercept same-origin GET /api/* requests with explicit respondWith(fetch())
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
// We also exclude SSE /api/events because Chrome keeps the SW alive for the entire
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
  if (!url.startsWith(self.location.origin + '/api/')) return;
  if (event.request.method !== 'GET') return;
  const path = url.slice(self.location.origin.length).split('?')[0];
  if (path === '/api/events') return;
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

// iOS doesn't reliably pass notification.data — fall back to the tag, which
// is set to the notification id at show time (the default tag means no id
// was set, e.g. a non-Lucidos push or a malformed payload).
const DEFAULT_NOTIFICATION_TAG = 'lucidos-notification';
function resolveNotificationId(notification) {
  const tag = notification.tag;
  return notification.data?.notification_id
    || (tag && tag !== DEFAULT_NOTIFICATION_TAG ? tag : null);
}

self.addEventListener('push', (event) => {
  let data = { title: 'Lucidos', body: 'New notification' };
  try {
    data = event.data.json();
  } catch {
    data.body = event.data?.text() || data.body;
  }

  event.waitUntil(
    Promise.all([
      self.registration.showNotification(data.title, {
        body: data.body,
        icon: '/favicon.svg',
        badge: '/favicon.svg',
        tag: data.notification_id || DEFAULT_NOTIFICATION_TAG,
        renotify: true,
        data: {
          notification_id: data.notification_id,
          app_id: data.app_id,
        },
      }),
      // Store the notification ID on the backend immediately — fallback for iOS
      // where notificationclick fires too late or not at all on warm resume.
      data.notification_id
        ? fetch(`${self.location.origin}/api/notification-pushed`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ notification_id: data.notification_id }),
          }).catch(() => {})
        : Promise.resolve(),
    ])
  );
});

// Clear the pending push fallback when the user dismisses — without this,
// /api/notification-pushed (60s window) fires the next time the app gains
// focus and auto-opens the modal for a notification the user just dismissed.
self.addEventListener('notificationclose', (event) => {
  const notificationId = resolveNotificationId(event.notification);
  if (!notificationId) return;
  event.waitUntil(
    fetch(`${self.location.origin}/api/notification-dismissed`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ notification_id: notificationId }),
    }).catch(() => {})
  );
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();

  const notificationId = resolveNotificationId(event.notification);
  const appId = event.notification.data?.app_id || null;

  event.waitUntil((async () => {
    // POST to backend — most reliable cross-platform mechanism.
    // Client-side storage (IDB, Cache API) and postMessage are unreliable
    // between SW and page on iOS Safari. A server round-trip works everywhere.
    if (notificationId) {
      try {
        await fetch(`${self.location.origin}/api/notification-clicked`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ notification_id: notificationId }),
        });
      } catch {}
    }

    const windowClients = await clients.matchAll({ type: 'window', includeUncontrolled: true });
    const appClient = windowClients.find(c =>
      !c.url.includes('/sw.js') && !c.url.includes('/api/skill/')
    );

    if (appClient) {
      let focused = appClient;
      try { focused = await appClient.focus(); } catch {}

      // postMessage for instant delivery when page is active (Chrome).
      // No navigate() — it causes page reload and Chrome's "site updated" toast.
      if (notificationId) {
        try {
          (focused || appClient).postMessage({
            type: 'open-notification',
            id: notificationId,
            app_id: appId,
          });
        } catch {}
      }
      return;
    }

    // No existing window — open a new one with deep-link URL param
    const base = self.location.origin;
    const params = new URLSearchParams();
    if (notificationId) params.set('notification', notificationId);
    if (appId) params.set('app', appId);
    const qs = params.toString();
    const targetUrl = qs ? `${base}/?${qs}` : `${base}/`;
    return clients.openWindow(targetUrl);
  })());
});
