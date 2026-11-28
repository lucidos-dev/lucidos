
import { createHmac } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { BrowserContext, Page } from '@playwright/test';
import { Agent, WebSocket } from 'undici';
import { E2E_WORKSPACE } from './address';

export const HEADER_GATEWAY_STAMP = 'x-lucidos-gateway-stamp';

const HEADER_DEVICE_ID = 'x-lucidos-device-id';
const DOMAIN = 'lucidos/gateway-stamp/v1';

export function signDeviceStamp(
  keyHex: string,
  workspaceId: string,
  deviceId: string,
  atUnix: number,
): string {
  const mac = createHmac('sha256', Buffer.from(keyHex, 'hex'));
  for (const field of [DOMAIN, 'device', String(atUnix), workspaceId, deviceId]) {
    const bytes = Buffer.from(field, 'utf8');
    mac.update(`${bytes.length}:`);
    mac.update(bytes);
  }
  return `v1.${atUnix}.${mac.digest('hex')}`;
}

function harnessKey(): { keyHex: string; workspaceId: string } | null {
  try {
    const [keyHex, workspaceId] = readFileSync(
      resolve(E2E_WORKSPACE, '.lucidos/e2e-stamp-key'),
      'utf-8',
    ).split('\n');
    return keyHex && workspaceId ? { keyHex, workspaceId } : null;
  } catch {
    return null;
  }
}

export function deviceStamp(deviceId: string): string | null {
  const key = harnessKey();
  if (!key) return null;
  return signDeviceStamp(key.keyHex, key.workspaceId, deviceId, Math.floor(Date.now() / 1000));
}

export function withDeviceStamp(headers: Record<string, string>): Record<string, string> {
  const named = Object.entries(headers).find(([k]) => k.toLowerCase() === HEADER_DEVICE_ID)?.[1];
  const signed = Object.keys(headers).some((k) => k.toLowerCase() === HEADER_GATEWAY_STAMP);
  const stamp = named && !signed ? deviceStamp(named) : null;
  return stamp ? { ...headers, [HEADER_GATEWAY_STAMP]: stamp } : headers;
}

export const KEEPS_THE_SERVICE_WORKER = { serviceWorkers: 'allow' } as const;

const LOOPBACK_TLS = new Agent({ connect: { rejectUnauthorized: false } });

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
    const pending: (string | Buffer)[] = [];
    server.onopen = () => pending.splice(0).forEach((frame) => server.send(frame));
    server.onmessage = (event) =>
      route.send(typeof event.data === 'string' ? event.data : Buffer.from(event.data));
    server.onclose = (event) => route.close({ code: event.code, reason: event.reason });
    route.onMessage((frame) =>
      server.readyState === WebSocket.CONNECTING ? pending.push(frame) : server.send(frame),
    );
    route.onClose((code, reason) =>
      code === 1000 || (code !== undefined && code >= 3000 && code <= 4999)
        ? server.close(code, reason)
        : server.close(),
    );
  });
}

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
