
import fs from 'node:fs';
import https from 'node:https';
import { sessionAdmits, type PreviewGateway } from './frontendPreviewGateway';

export function loopbackAgent(gw: PreviewGateway): https.Agent {
  return new https.Agent({ ca: fs.readFileSync(gw.caPath) });
}

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
