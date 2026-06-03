import { apiUrl } from './_fetch';

/**
 * Generic API proxy. Configure backends in `data/config/apis.json`:
 *
 * ```json
 * {
 *   "sonos":   { "base_url": "http://localhost:5005" },
 *   "comfort": { "base_url": "https://accsmart.panasonic.com",
 *                "auth": { "type": "bearer", "credential": "comfort-cloud" } }
 * }
 * ```
 *
 * Then from an app:
 *
 * ```ts
 * lucidos.proxy('sonos').fetch('/Spisestua/play');
 * lucidos.proxy('comfort').fetch('/api/v1/devices', { method: 'POST', body });
 * ```
 *
 * The engine forwards the request to the configured backend, strips
 * Cookie/Origin/Referer/Host (so the upstream doesn't see the engine's
 * browser session), and injects the configured auth header from the
 * credential store. The credential value never reaches the iframe.
 */
export interface ProxyClient {
  /** Make a request to the configured backend. Returns the raw `Response`
   *  so the caller can decide how to read the body (`.json()`, `.text()`,
   *  `.blob()`, …). */
  fetch(path: string, init?: RequestInit): Promise<Response>;
}

export function proxy(name: string): ProxyClient {
  const safeName = encodeURIComponent(name);
  return {
    fetch(path: string, init?: RequestInit): Promise<Response> {
      const normalizedPath = path.startsWith('/') ? path : `/${path}`;
      return fetch(apiUrl(`/proxy/${safeName}${normalizedPath}`), init);
    },
  };
}
