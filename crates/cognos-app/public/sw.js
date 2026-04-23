// CognOS Service Worker — handles push notifications and PWA support

// Activate new SW immediately and take control of all pages.
// This triggers Chrome's "site updated" toast when sw.js changes,
// but only during development — in production SW updates are rare.
self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (event) => event.waitUntil(clients.claim()));

// Required for iOS PWA — without a fetch listener iOS won't treat this as a valid SW.
// IMPORTANT: We explicitly call event.respondWith(fetch(request)) for same-origin
// API requests instead of relying on the implicit "don't call respondWith" fallback.
// iOS Safari (WebKit) can lose the implicit fallback path after the SW is killed and
// restarted under memory pressure, causing fetch() to return empty/corrupted responses.
// Non-API requests (navigation, cross-origin, static assets) use the implicit fallback
// since they don't need special handling and respondWith can break them (opaque responses).
self.addEventListener('fetch', (event) => {
  const url = event.request.url;
  if (url.startsWith(self.location.origin + '/api/')) {
    const path = url.slice(self.location.origin.length).split('?')[0];
    if (path !== '/api/events') {
      event.respondWith(fetch(event.request));
    }
  }
  // For everything else (including SSE /api/events): don't call respondWith —
  // browser handles normally. SSE must NOT go through the SW because Chrome keeps
  // the SW alive for the entire streaming connection, causing hangs when two SW
  // versions coexist (active + waiting).
});

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
        tag: data.notification_id || 'cognos-notification',
        renotify: true,
        data: {
          notification_id: data.notification_id,
          app_id: data.app_id,
          thread_id: data.thread_id,
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

self.addEventListener('notificationclick', (event) => {
  event.notification.close();

  // iOS doesn't reliably pass notification.data — fall back to tag
  const tag = event.notification.tag;
  const notificationId = event.notification.data?.notification_id
    || (tag && tag !== 'cognos-notification' ? tag : null);
  const appId = event.notification.data?.app_id || null;
  const threadId = event.notification.data?.thread_id || null;

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
      if (notificationId || threadId) {
        try {
          (focused || appClient).postMessage({
            type: 'open-notification',
            id: notificationId,
            app_id: appId,
            thread_id: threadId,
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
    if (threadId) params.set('thread', threadId);
    const qs = params.toString();
    const targetUrl = qs ? `${base}/?${qs}` : `${base}/`;
    return clients.openWindow(targetUrl);
  })());
});
