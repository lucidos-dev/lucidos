/**
 * Ask the workspace gateway whether a browser cookie belongs to a paired device,
 * for the frontend preview's auth gate (`frontendPreviewGateway.ts`).
 *
 * Its own module because it needs Node's `fs` and `https`, and the app's
 * `tsconfig` carries no Node types. Only `vite.config.ts` imports it, and that
 * file is not type-checked with the app.
 */

import fs from 'node:fs';
import https from 'node:https';
import { sessionAdmits, type PreviewGateway } from './frontendPreviewGateway';

/**
 * The agent for every preview hop to the gateway. It trusts only the loopback
 * CA, like `lucidos_local_token::client`, so a squatter on the port fails.
 */
export function loopbackAgent(gw: PreviewGateway): https.Agent {
  return new https.Agent({ ca: fs.readFileSync(gw.caPath) });
}

/**
 * Resolves `true` only for a paired device, and `false` on any failure. Sends
 * the browser's `Cookie` header and nothing else.
 */
export function askGatewaySession(
  gw: PreviewGateway,
  agent: https.Agent,
  cookie: string,
): Promise<boolean> {
  const url = new URL('/~/api/v1/auth/session', gw.origin);
  return new Promise((resolve) => {
    const req = https.request(
      url,
      { method: 'GET', headers: { cookie }, agent, timeout: 5000 },
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
