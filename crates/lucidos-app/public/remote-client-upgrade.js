// Executed only in A's trusted picker/upgrade document, before navigation.
// Cache names were origin-wide in older clients. Remove remote requests, never
// entire caches: local workspaces may share the same shell/blob cache.
window.lucidosRemoteClientReady = (async function () {
  function remotePath(url) {
    var parsed = new URL(url, location.origin);
    return parsed.origin === location.origin && /^\/[a-z0-9](?:[a-z0-9-]*[a-z0-9])?@[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\//.test(parsed.pathname);
  }
  function trustedWorker(registration) {
    var workers = [registration.active, registration.waiting, registration.installing].filter(Boolean);
    return workers.length > 0 && workers.every(function (worker) {
      var script = new URL(worker.scriptURL);
      var workspace = script.searchParams.get('workspace');
      return script.origin === location.origin && script.pathname === '/~/sw.js'
        && workspace === new URL(registration.scope).pathname.slice(1, -1);
    });
  }
  if ('serviceWorker' in navigator) {
    var registrations = await navigator.serviceWorker.getRegistrations();
    for (var registration of registrations) {
      if (remotePath(registration.scope) && !trustedWorker(registration) && !await registration.unregister()) {
        // Another trusted tab may already have retired the same registration.
        var remaining = await navigator.serviceWorker.getRegistrations();
        if (remaining.some(function (item) { return item.scope === registration.scope; })) {
          throw new Error('Could not retire the old remote workspace worker. Close remote workspace tabs and retry.');
        }
      }
    }
  }
  if (typeof caches !== 'undefined') {
    for (var name of await caches.keys()) {
      var cache = await caches.open(name);
      for (var request of await cache.keys()) {
        if (remotePath(request.url) && !await cache.delete(request) && await cache.match(request)) {
          throw new Error('Could not retire an old remote workspace cache. Retry from Manage workspaces.');
        }
      }
    }
  }
  // Migration marker only, never an authentication or authorization credential.
  // Session scoped: repeat retirement after reopening the browser.
  document.cookie = 'lucidos-remote-client=1; Path=/; SameSite=Strict' + (location.protocol === 'https:' ? '; Secure' : '');
})();
// Keep rejection handled even before the boot/cold-start consumers attach.
window.lucidosRemoteClientReady.catch(function (error) { console.error('[Remote client upgrade]', error); });
