self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', event => event.waitUntil(self.clients.claim()));
self.addEventListener('fetch', event => {
  if (event.request.mode === 'navigate') {
    event.respondWith(new Response('<title>foreign cached shell</title>', {
      headers: {'Content-Type': 'text/html'},
    }));
  }
});
