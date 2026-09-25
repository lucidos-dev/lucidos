/**
 * Ask the workspace gateway whether a browser cookie belongs to a paired device,
 * for the frontend preview's auth gate (`frontendPreviewGateway.ts`).
 *
 * Its own module because it needs Node's `http` / `https`, and the app's
 * `tsconfig` carries no Node types. Only `vite.config.ts` imports it, and that
 * file is not type-checked with the app.
 */

import http from 'node:http';
import https from 'node:https';
import { sessionAdmits, type PreviewGateway } from './frontendPreviewGateway';

/**
 * Resolves `true` only for a paired device, and `false` on any failure. Sends
 * the browser's `Cookie` header and nothing else. The self-signed dev cert is
 * accepted, as on every intra-host Lucidos hop.
 */
export function askGatewaySession(gw: PreviewGateway, cookie: string): Promise<boolean> {
  const url = new URL('/~/api/v1/auth/session', gw.origin);
  const client = url.protocol === 'https:' ? https : http;
  return new Promise((resolve) => {
    const req = client.request(
      url,
      { method: 'GET', headers: { cookie }, rejectUnauthorized: false, timeout: 5000 },
      (res) => {
        let raw = '';
        res.setEncoding('utf8');
        res.on('data', (chunk) => (raw += chunk));
        res.on('end', () => {
          try {
            resolve(sessionAdmits(res.statusCode ?? 0, JSON.parse(raw)));
          } catch {
            resolve(false);
          }
        });
      },
    );
    req.on('timeout', () => req.destroy());
    req.on('error', () => resolve(false));
    req.end();
  });
}
