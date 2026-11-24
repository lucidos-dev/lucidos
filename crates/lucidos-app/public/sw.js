// Temporary worker instance mark: docs/temporary-measures.md, Page health log.
const WORKER_INSTANCE = Math.floor(Math.random() * Number.MAX_SAFE_INTEGER);

const SCOPE_PATH = (() => {
  try {
    return new URL(self.registration.scope).pathname;
  } catch {
    return '/';
  }
})();

const IS_REMOTE_SCOPE = SCOPE_PATH.includes('@');

function scopeRelative(path) {
  return path.startsWith(SCOPE_PATH) ? '/' + path.slice(SCOPE_PATH.length) : path;
}

function notificationGeneration(data) {
  return data.restore_generation === null ? 'none'
    : typeof data.restore_generation === 'string' ? data.restore_generation : 'unknown';
}

function swLog(message, data, generation = 'unknown') {
  try {
    return fetch(`${self.location.origin}${SCOPE_PATH}api/v1/internal/client-log`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'x-lucidos-restore-generation': generation },
      body: JSON.stringify({ category: 'sw', code: message, message, data: data || {} }),
      keepalive: true,
    }).catch(() => {});
  } catch {
    return Promise.resolve();
  }
}

self.addEventListener('install', (event) => {
  self.skipWaiting();
  if (IS_BUILT && !IS_REMOTE_SCOPE) {
    event.waitUntil((async () => {
      try {
        const shellKey = new Request(self.location.origin + SCOPE_PATH);
        const resp = await fetch(self.location.origin + SCOPE_PATH, { cache: 'reload' });
        if (resp && resp.ok && !resp.redirected && !isPairingShell(resp)) {
          const cache = await caches.open(SHELL_CACHE);
          await cache.put(shellKey, resp.clone());
        }
      } catch {
      }
    })());
  }
});
self.addEventListener('activate', (event) => {
  event.waitUntil(
    (async () => {
      if (!IS_REMOTE_SCOPE) {
        const names = await caches.keys();
        await Promise.all(
          names.filter((name) => !KEEP_CACHES.includes(name)).map((name) => caches.delete(name)),
        );
      }
      await clients.claim();
    })(),
  );
});

const BLOB_CACHE = 'lucidos-blob-v1';

const BUILD_ID = '__LUCIDOS_BUILD_ID__';

const SHELL_CACHE = 'lucidos-shell-' + BUILD_ID;

const IS_BUILT = !BUILD_ID.startsWith('__');

const KEEP_CACHES = [BLOB_CACHE, SHELL_CACHE];

const FETCH_PROBE_PARAM = 'sw-fetch-probe';

self.addEventListener('fetch', (event) => {
  if (IS_REMOTE_SCOPE) return;
  if (event.request.method !== 'GET') return;
  const url = event.request.url;
  if (!url.startsWith(self.location.origin)) return;
  const path = url.slice(self.location.origin.length).split('?')[0];
  const rel = scopeRelative(path);

  if (rel.startsWith('/api/v1/')) {
    if (rel.startsWith('/api/v1/blobs/')) {
      event.respondWith(cacheFirst(event.request, BLOB_CACHE));
      return;
    }
    if (rel === '/api/v1/health' && new URL(url).searchParams.has(FETCH_PROBE_PARAM)) {
      event.respondWith(fetchWithRetry(event.request));
    }
    return;
  }

  if (rel.startsWith('/data/') && event.request.destination === 'image') {
    event.respondWith(fetchWhole(event.request));
    return;
  }

  if (rel.startsWith('/assets/')) {
    const response = cacheFirst(event.request, SHELL_CACHE, (r) => !!r && r.ok && !isHtmlResponse(r), fetchWhole);
    event.respondWith(response);
    event.waitUntil(response.then((r) => logFailedAsset(rel, r)).catch(() => {}));
    return;
  }

  if (IS_BUILT && event.request.mode === 'navigate' && rel === '/') {
    event.respondWith(networkFirstShell(event.request));
    return;
  }
});

async function fetchWithRetry(request, init) {
  try {
    return await fetch(request, init);
  } catch {
    return await fetch(request, init);
  }
}

const WHOLE_BODY_ATTEMPTS = 3;
const WHOLE_BODY_STALL_MS = 10_000;

async function fetchWhole(request) {
  for (let attempt = 1; attempt <= WHOLE_BODY_ATTEMPTS; attempt++) {
    try {
      return await readWhole(request, attempt === 1 ? {} : { cache: 'reload' });
    } catch {
    }
  }
  return Response.error();
}

async function readWhole(request, init) {
  const controller = new AbortController();
  let stallTimer = null;
  const armStallTimer = () => {
    clearTimeout(stallTimer);
    stallTimer = setTimeout(() => controller.abort(), WHOLE_BODY_STALL_MS);
  };
  armStallTimer();
  try {
    const response = await fetch(request, { ...init, signal: controller.signal });
    if (!response.ok || !response.body) return response;
    const chunks = [];
    let received = 0;
    const reader = response.body.getReader();
    for (;;) {
      armStallTimer();
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
      received += value.byteLength;
    }
    const declared = Number(response.headers.get('content-length'));
    if (!response.headers.has('content-encoding') && declared > 0 && received !== declared) {
      throw new Error(`body ended at ${received} of ${declared} bytes`);
    }
    const headers = new Headers(response.headers);
    headers.delete('content-encoding');
    headers.set('content-length', String(received));
    return new Response(new Blob(chunks), {
      status: response.status,
      statusText: response.statusText,
      headers,
    });
  } finally {
    clearTimeout(stallTimer);
  }
}

function logFailedAsset(rel, response) {
  if (response.ok && !isHtmlResponse(response)) return undefined;
  return swLog('asset_failed', {
    path: rel,
    status: response.status,
    type: response.type,
    contentType: response.headers.get('content-type'),
  });
}

function isHtmlResponse(response) {
  const ct = response && response.headers && response.headers.get('content-type');
  return typeof ct === 'string' && ct.includes('text/html');
}

function isPairingShell(response) {
  return !!(response && response.headers && response.headers.get('x-lucidos-pairing'));
}

async function cacheFirst(request, cacheName, isCacheable, fetchMiss = fetchWithRetry) {
  const cache = await caches.open(cacheName);
  const cached = await cache.match(request);
  if (cached) return cached;
  const response = await fetchMiss(request);
  const cacheable = isCacheable ? isCacheable(response) : (response && response.ok);
  if (cacheable) {
    cache.put(request, response.clone()).catch(() => {});
  }
  return response;
}

async function networkFirstShell(request) {
  const cache = await caches.open(SHELL_CACHE);
  const shellKey = new Request(self.location.origin + SCOPE_PATH);
  let response;
  try {
    response = await fetchWithRetry(request, { cache: 'no-cache' });
  } catch (err) {
    const cached = await cache.match(shellKey);
    if (cached) return cached;
    throw err;
  }
  if (isPairingShell(response)) {
    return response;
  }
  if (response && response.ok && !response.redirected) {
    cache.put(shellKey, response.clone()).catch(() => {});
    return response;
  }
  if (response && response.redirected && response.url) {
    return Response.redirect(response.url, 302);
  }
  if (response && (response.status === 503 || response.headers.get('x-lucidos-boot-splash'))) {
    return response;
  }
  const cached = await cache.match(shellKey);
  return cached || response;
}

const DEFAULT_NOTIFICATION_TAG = 'lucidos-notification';

const UNWATCH_PUSH_ACTION = 'unwatch';

function resolveNavigate(relativeUrl) {
  if (typeof relativeUrl !== 'string' || relativeUrl.length === 0) {
    return self.location.origin + SCOPE_PATH;
  }
  try {
    const scopeRelativeUrl = relativeUrl.replace(/^\//, '');
    const target = new URL(scopeRelativeUrl, self.location.origin + SCOPE_PATH);
    if (IS_REMOTE_SCOPE && (target.origin !== self.location.origin || target.pathname !== SCOPE_PATH)) {
      return self.location.origin + SCOPE_PATH;
    }
    return target.toString();
  } catch {
    return self.location.origin + SCOPE_PATH;
  }
}

self.addEventListener('push', (event) => {
  let raw = null;
  let textBody;
  try {
    raw = event.data.json();
  } catch {
    textBody = event.data?.text();
  }

  const isDeclarative =
    raw && typeof raw === 'object' && raw.web_push === 8030
    && raw.notification && typeof raw.notification === 'object';
  const notification = isDeclarative ? raw.notification : { body: textBody };

  const isWake = raw?.wake === true;

  const title = notification.title || 'Lucidos';
  const body = notification.body || 'New notification';
  const tag = notification.tag || DEFAULT_NOTIFICATION_TAG;
  const navigateRelative = notification.navigate;

  const data = notification.data || {};

  const appBadge =
    raw && typeof raw.app_badge === 'number' ? raw.app_badge : null;

  swLog('push', {
    declarative: isDeclarative,
    wake: isWake,
    has_navigate: !!navigateRelative,
    notification_id: (data && data.notification_id) || null,
  }, notificationGeneration(data));
  event.waitUntil(self.registration.showNotification(title, {
    body,
    icon: SCOPE_PATH + 'favicon.svg',
    badge: SCOPE_PATH + 'favicon.svg',
    tag,
    renotify: !isWake,
    requireInteraction: true,
    silent: isWake,
    actions: Array.isArray(notification.actions)
      ? (IS_REMOTE_SCOPE
        ? notification.actions.filter(a => a?.action === UNWATCH_PUSH_ACTION && typeof a.title === 'string')
          .map(a => ({action: UNWATCH_PUSH_ACTION, title: a.title.slice(0, 80)}))
        : notification.actions)
      : [],
    navigate: navigateRelative
      ? resolveNavigate(navigateRelative)
      : self.location.origin + SCOPE_PATH,
    data,
  }));

  if (appBadge !== null && typeof self.navigator.setAppBadge === 'function') {
    const badgePromise =
      appBadge > 0 ? self.navigator.setAppBadge(appBadge) : self.navigator.clearAppBadge?.();
    if (badgePromise) event.waitUntil(badgePromise.catch(() => {}));
  }
});

self.addEventListener('message', (event) => {
  if (event.data?.type === 'lucidos:ping' && event.source) {
    event.source.postMessage({ type: 'lucidos:pong', instance: WORKER_INSTANCE });
    return;
  }
  if (event.data?.type === 'lucidos:get-build-id' && event.source) {
    event.source.postMessage({ type: 'lucidos:build-id', buildId: BUILD_ID });
  }
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();

  const data = event.notification.data || {};
  const notificationId = data.notification_id || null;
  const generation = notificationGeneration(data);

  const targetUrl = data.navigate
    ? resolveNavigate(data.navigate)
    : self.location.origin + SCOPE_PATH;

  if (event.action === UNWATCH_PUSH_ACTION && data.unwatch_thread_id) {
    const headers = { 'x-lucidos-restore-generation': generation };
    const url = (path) => `${self.location.origin}${SCOPE_PATH}api/v1/${path}`;
    const threadId = encodeURIComponent(data.unwatch_thread_id);
    const unwatch = fetch(url(`threads/${threadId}/users/unwatch`), { method: 'POST', headers });
    const read = notificationId
      ? fetch(url(`notification/read?id=${encodeURIComponent(notificationId)}`), { method: 'POST', headers })
      : Promise.resolve();
    event.waitUntil(Promise.all([unwatch, read]).catch(() => {}));
    return;
  }

  const markReadPromise = notificationId
    ? fetch(`${self.location.origin}${SCOPE_PATH}api/v1/notification/read?id=${encodeURIComponent(notificationId)}`, {
        method: 'POST',
        headers: { 'x-lucidos-restore-generation': generation },
      }).catch(() => {})
    : Promise.resolve();

  const logPromise = swLog('notificationclick', {
    notification_id: notificationId,
    has_navigate: !!data.navigate,
    target_url: targetUrl.slice(0, 200),
  }, generation);

  event.waitUntil(Promise.all([logPromise, markReadPromise, routeToDeepLink(targetUrl, data)]));
});

function isWorkspaceShell(clientUrl) {
  let pathname;
  try {
    pathname = new URL(clientUrl).pathname;
  } catch {
    return false;
  }
  return pathname === SCOPE_PATH || pathname + '/' === SCOPE_PATH;
}

async function routeToDeepLink(targetUrl, tapData) {
  const windowClients = await clients.matchAll({ type: 'window', includeUncontrolled: true });
  const shellClient = windowClients.find(c =>
    c.frameType === 'top-level' && isWorkspaceShell(c.url)
  );

  if (shellClient) {
    await shellClient.focus().catch(() => {});
    shellClient.postMessage({ type: 'lucidos:deep-link', target: tapData });
    return;
  }
  await clients.openWindow(targetUrl);
}
