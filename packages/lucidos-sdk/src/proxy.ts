import { apiUrl } from './_fetch';
import {
  callHost,
  fromWireResponse,
  headersToRecord,
  isBridged,
  toWireBody,
  type WireRequest,
  type WireResponse,
} from './_bridge';

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
 * lucidos.proxy('sonos').fetch('/living-room/play');
 * lucidos.proxy('comfort').fetch('/api/v1/devices', { method: 'POST', body });
 * ```
 *
 * The engine forwards the request to the configured backend and injects the
 * configured auth header from the credential store. It strips
 * Cookie/Origin/Referer/Host, so the upstream never sees the engine's browser
 * session. It strips every `x-lucidos-*` header and the two `x-forwarded-*`
 * ones the gateway owns with them, so no Lucidos credential travels either.
 * The credential value never reaches the iframe.
 */
export interface ProxyClient {
  /** Make a request to the configured backend. Returns the raw `Response`
   *  so the caller can decide how to read the body (`.json()`, `.text()`,
   *  `.blob()`, …). */
  fetch(path: string, init?: RequestInit): Promise<Response>;
}

/** How long the host will wait on a proxied upstream before giving up.
 *
 *  The engine owns the real limit: `proxy_timeout_secs`, or an entry's
 *  `timeout_secs`, with one proxied call capped at 600 s. This sits a minute
 *  above that cap, so it never cuts first. It exists so a hung backend cannot
 *  leave a pending entry for the life of the frame. An engine test pins it. */
const PROXY_TIMEOUT_MS = 660000;

export function proxy(name: string): ProxyClient {
  const safeName = encodeURIComponent(name);
  return {
    async fetch(path: string, init?: RequestInit): Promise<Response> {
      const normalizedPath = path.startsWith('/') ? path : `/${path}`;
      const suffix = `/proxy/${safeName}${normalizedPath}`;
      if (!isBridged()) return fetch(apiUrl(suffix), init);
      // An isolated frame's own `fetch` is CORS-blocked, so the host makes the
      // call. The raw `Response` is rebuilt on this side, so a caller still
      // picks its own way to read the body.
      const wire: WireRequest = {
        path: suffix,
        method: init?.method ?? 'GET',
        headers: headersToRecord(init?.headers),
        body: toWireBody(init?.body),
        timeoutMs: PROXY_TIMEOUT_MS,
      };
      const value = await callHost('fetch', wire, PROXY_TIMEOUT_MS);
      return fromWireResponse(value as WireResponse);
    },
  };
}
