/**
 * The browser harness signs its own device stamps.
 *
 * An engine trusts `x-lucidos-device-id` only with a gateway stamp, signed by
 * the key its starter wrote to its stdin. The suite drives the e2e engine
 * directly, with no gateway, so the harness is the starter
 * (`scripts/lib/e2e_stamp.sh`) and signs here what a gateway would.
 *
 * The format is `crates/lucidos-gateway-stamp`. `harnessStamp.test.ts` pins
 * the same vector as that crate's `a_fixed_key_signs_the_pinned_vector`.
 */

import { createHmac } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { BrowserContext, Page } from '@playwright/test';
import { Agent, WebSocket } from 'undici';
import { E2E_WORKSPACE } from './address';

/** `HEADER_GATEWAY_STAMP` in `crates/lucidos-gateway-stamp`. */
export const HEADER_GATEWAY_STAMP = 'x-lucidos-gateway-stamp';

const HEADER_DEVICE_ID = 'x-lucidos-device-id';
const DOMAIN = 'lucidos/gateway-stamp/v1';

/** The stamp for a device claim, at `atUnix` seconds. Pure. */
export function signDeviceStamp(
  keyHex: string,
  workspaceId: string,
  deviceId: string,
  atUnix: number,
): string {
  const mac = createHmac('sha256', Buffer.from(keyHex, 'hex'));
  // Each field is length-prefixed in bytes, so no value can shift a boundary.
  for (const field of [DOMAIN, 'device', String(atUnix), workspaceId, deviceId]) {
    const bytes = Buffer.from(field, 'utf8');
    mac.update(`${bytes.length}:`);
    mac.update(bytes);
  }
  return `v1.${atUnix}.${mac.digest('hex')}`;
}

/** The key file holds the key in hex, then the workspace id its stamps name.
 *  Read per call: the harness mints a new key each time it starts an engine. */
function harnessKey(): { keyHex: string; workspaceId: string } | null {
  try {
    const [keyHex, workspaceId] = readFileSync(
      resolve(E2E_WORKSPACE, '.lucidos/e2e-stamp-key'),
      'utf-8',
    ).split('\n');
    return keyHex && workspaceId ? { keyHex, workspaceId } : null;
  } catch {
    // An engine the harness did not start: every device stays unattested.
    return null;
  }
}

/** A stamp for `deviceId`, signed now, or null with no harness key. */
export function deviceStamp(deviceId: string): string | null {
  const key = harnessKey();
  if (!key) return null;
  return signDeviceStamp(key.keyHex, key.workspaceId, deviceId, Math.floor(Date.now() / 1000));
}

/** `headers` plus a fresh stamp for the device they name. Unchanged when they
 *  name none, or when they already carry a stamp. */
export function withDeviceStamp(headers: Record<string, string>): Record<string, string> {
  const named = Object.entries(headers).find(([k]) => k.toLowerCase() === HEADER_DEVICE_ID)?.[1];
  const signed = Object.keys(headers).some((k) => k.toLowerCase() === HEADER_GATEWAY_STAMP);
  const stamp = named && !signed ? deviceStamp(named) : null;
  return stamp ? { ...headers, [HEADER_GATEWAY_STAMP]: stamp } : headers;
}

/** For `test.use` in a spec that needs the service worker on WebKit, where
 *  the project blocks it (`playwright.config.ts`). Its sends after the
 *  worker takes control arrive unstamped, so it must not assert who sent
 *  them. */
export const KEEPS_THE_SERVICE_WORKER = { serviceWorkers: 'allow' } as const;

/** The e2e engine serves https with a self-signed loopback certificate when
 *  the checkout has dev certs. The browser accepts it through
 *  `ignoreHTTPSErrors`; the relay below needs its own opt-out. */
const LOOPBACK_TLS = new Agent({ connect: { rejectUnauthorized: false } });

/** Stamp the sockets `page` opens to `socketUrl`, as the gateway would on an
 *  upgrade. A browser cannot put a header on a `WebSocket`, so the route
 *  dials the engine itself with the stamp and relays frames both ways.
 *  The stamp names the device the page's own API requests name. */
export async function stampPageWebSockets(page: Page, socketUrl: RegExp): Promise<void> {
  let deviceId = '';
  page.on('request', (request) => {
    deviceId = request.headers()[HEADER_DEVICE_ID] ?? deviceId;
  });
  await page.routeWebSocket(socketUrl, (route) => {
    const stamp = deviceId ? deviceStamp(deviceId) : null;
    const headers: Record<string, string> = deviceId
      ? { [HEADER_DEVICE_ID]: deviceId, ...(stamp ? { [HEADER_GATEWAY_STAMP]: stamp } : {}) }
      : {};
    const server = new WebSocket(route.url(), { headers, dispatcher: LOOPBACK_TLS });
    server.binaryType = 'arraybuffer';
    // Frames the page sends before the engine accepts the upgrade wait here.
    const pending: (string | Buffer)[] = [];
    server.onopen = () => pending.splice(0).forEach((frame) => server.send(frame));
    server.onmessage = (event) =>
      route.send(typeof event.data === 'string' ? event.data : Buffer.from(event.data));
    server.onclose = (event) => route.close({ code: event.code, reason: event.reason });
    route.onMessage((frame) =>
      server.readyState === WebSocket.CONNECTING ? pending.push(frame) : server.send(frame),
    );
    // A client may send only 1000 or 3000-4999. Reserved codes such as 1005
    // ("no code") describe the close and cannot be sent, so they close plainly.
    route.onClose((code, reason) =>
      code === 1000 || (code !== undefined && code >= 3000 && code <= 4999)
        ? server.close(code, reason)
        : server.close(),
    );
  });
}

/** Stamp every API request a page in `context` sends, as the gateway would.
 *  The `context` fixture is stamped already. A context a spec opens itself
 *  with `browser.newContext` needs this call. A spec's own `page.route` runs
 *  first. It must pass a request on with
 *  `route.fallback()`: `route.continue()` skips this route, so the request
 *  goes out unstamped. */
export async function stampPageRequests(context: BrowserContext): Promise<void> {
  await context.route(
    (url) => url.pathname.includes('/api/v1/'),
    async (route) => {
      const headers = route.request().headers();
      const stamped = withDeviceStamp(headers);
      await (stamped === headers ? route.fallback() : route.fallback({ headers: stamped }));
    },
  );
}
