import { describe, it, expect } from 'vitest';
import {
  cachedVerdict,
  gateUpgrades,
  isProxiedPath,
  previewAuthMiddleware,
  previewGatewayFromEnv,
  previewProxy,
  sessionAdmits,
  PREVIEW_GATEWAY_ORIGIN_ENV,
  PREVIEW_PROXIED_PREFIXES,
  PREVIEW_WORKSPACE_ID_ENV,
  type GateRequest,
  type GateResponse,
  type PreviewGateway,
  type UpgradeServer,
} from '../../vite/frontendPreviewGateway';

/**
 * The frontend preview's gateway wiring (ADR 0267). The module under test is
 * build config under `crates/lucidos-app/vite/`, deliberately out of the app
 * bundle. It is tested from here because Vitest's `include` covers `src/` only.
 */
const GW: PreviewGateway = { origin: 'https://127.0.0.1:5251', workspaceId: 'dev' };

describe('previewGatewayFromEnv', () => {
  const env = (origin?: string, slug?: string) => ({
    [PREVIEW_GATEWAY_ORIGIN_ENV]: origin,
    [PREVIEW_WORKSPACE_ID_ENV]: slug,
  });

  it('is absent without both engine-supplied values, so `npm run dev` is untouched', () => {
    expect(previewGatewayFromEnv({})).toBeUndefined();
    expect(previewGatewayFromEnv(env(GW.origin, undefined))).toBeUndefined();
    expect(previewGatewayFromEnv(env(undefined, 'dev'))).toBeUndefined();
    expect(previewGatewayFromEnv(env('  ', 'dev'))).toBeUndefined();
  });

  it('refuses a workspace id that is not slug-shaped, so it never becomes a path', () => {
    for (const slug of ['../dev', 'dev/x', 'Dev', '-dev', '']) {
      expect(previewGatewayFromEnv(env(GW.origin, slug))).toBeUndefined();
    }
  });

  it('reads the gateway the engine named', () => {
    expect(previewGatewayFromEnv(env(GW.origin, 'dev'))).toEqual(GW);
  });

  it('carries the same env-var names the engine sets', () => {
    // Mirrored in `engine::frontend_preview`. A drift between the two is
    // silent: the preview just never appears.
    expect(PREVIEW_GATEWAY_ORIGIN_ENV).toBe('LUCIDOS_FRONTEND_PREVIEW_GATEWAY_ORIGIN');
    expect(PREVIEW_WORKSPACE_ID_ENV).toBe('LUCIDOS_FRONTEND_PREVIEW_WORKSPACE_ID');
  });
});

describe('previewProxy', () => {
  it('sends the bundle prefixes to the workspace on the gateway, never to the engine', () => {
    const proxy = previewProxy(GW);
    for (const prefix of PREVIEW_PROXIED_PREFIXES) {
      expect(proxy[prefix]).toEqual({
        target: 'https://127.0.0.1:5251/dev',
        changeOrigin: true,
        // The dev gateway serves its own self-signed cert.
        secure: false,
      });
    }
  });

  it('passes the slug prefix through unchanged, so app frames load', () => {
    expect(previewProxy(GW)['/dev/']).toEqual({
      target: 'https://127.0.0.1:5251',
      changeOrigin: true,
      secure: false,
    });
  });

  it('forwards exactly those four prefixes and adds no header of its own', () => {
    const proxy = previewProxy(GW);
    expect(Object.keys(proxy).sort()).toEqual(['/api', '/app', '/data', '/dev/']);
    // A `headers` key could carry the machine-local token, which would make
    // every LAN caller a local process. Only the browser's cookie may travel.
    for (const entry of Object.values(proxy)) {
      expect(Object.keys(entry).sort()).toEqual(['changeOrigin', 'secure', 'target']);
    }
  });
});

describe('sessionAdmits', () => {
  it('admits only a paired device', () => {
    expect(sessionAdmits(200, { paired: true, device_id: 'd1', local: false })).toBe(true);
  });

  it('refuses everything else, and an unreadable answer is never a yes', () => {
    expect(sessionAdmits(200, { paired: false, local: false })).toBe(false);
    expect(sessionAdmits(200, { paired: true, local: true })).toBe(false);
    expect(sessionAdmits(200, { paired: true, device_id: 'd1', local: true })).toBe(false);
    expect(sessionAdmits(200, { paired: true })).toBe(false);
    expect(sessionAdmits(500, { paired: true, device_id: 'd1' })).toBe(false);
    expect(sessionAdmits(200, null)).toBe(false);
    expect(sessionAdmits(200, 'paired')).toBe(false);
  });
});

describe('isProxiedPath', () => {
  it('matches the proxy by prefix, exactly as Vite does', () => {
    expect(isProxiedPath(GW, '/api/v1/threads/list')).toBe(true);
    expect(isProxiedPath(GW, '/data/artifacts/x.png')).toBe(true);
    expect(isProxiedPath(GW, '/dev/~cap/t/app/x/')).toBe(true);
    expect(isProxiedPath(GW, '/')).toBe(false);
    expect(isProxiedPath(GW, '/@fs/ws/WORKSPACES.md')).toBe(false);
    expect(isProxiedPath(GW, '/src/main.tsx')).toBe(false);
    expect(isProxiedPath(GW, '/devx')).toBe(false);
  });
});

describe('previewAuthMiddleware', () => {
  type Middleware = ReturnType<typeof previewAuthMiddleware>;
  interface Outcome { passed: boolean; status?: number }

  function run(mw: Middleware, url: string, cookie?: string): Promise<Outcome> {
    return new Promise((resolve) => {
      const req: GateRequest = { url, headers: cookie ? { cookie } : {} };
      const res: GateResponse = {
        statusCode: 200,
        setHeader: () => {},
        end() { resolve({ passed: false, status: res.statusCode }); },
      };
      mw(req, res, () => resolve({ passed: true }));
    });
  }

  function gate(admitted: string[]) {
    const asked: string[] = [];
    const verdict = cachedVerdict(async (cookie) => {
      asked.push(cookie);
      return admitted.includes(cookie);
    });
    return { mw: previewAuthMiddleware(GW, verdict), asked };
  }

  it('refuses a Vite-served file with no cookie, and asks nobody', async () => {
    const { mw, asked } = gate([]);
    for (const url of ['/', '/@fs/ws/WORKSPACES.md', '/@vite/client', '/src/main.tsx']) {
      expect(await run(mw, url)).toEqual({ passed: false, status: 401 });
    }
    expect(asked).toEqual([]);
  });

  it('refuses a cookie the gateway does not know', async () => {
    const { mw } = gate([]);
    expect(await run(mw, '/', 'lucidos_device_x=stale')).toEqual({ passed: false, status: 401 });
  });

  it('refuses when asking the gateway fails', async () => {
    const mw = previewAuthMiddleware(
      GW,
      cachedVerdict(() => Promise.reject(new Error('gateway down'))),
    );
    expect(await run(mw, '/', 'lucidos_device_x=ok')).toEqual({ passed: false, status: 401 });
  });

  it('admits a paired device, and asks once per cookie', async () => {
    const { mw, asked } = gate(['lucidos_device_x=ok']);
    expect(await run(mw, '/', 'lucidos_device_x=ok')).toEqual({ passed: true });
    expect(await run(mw, '/src/main.tsx', 'lucidos_device_x=ok')).toEqual({ passed: true });
    expect(asked).toEqual(['lucidos_device_x=ok']);
  });

  it('leaves forwarded paths to the gateway, which checks the cookie itself', async () => {
    const { mw, asked } = gate([]);
    expect(await run(mw, '/api/v1/threads/list')).toEqual({ passed: true });
    expect(await run(mw, '/dev/api/v1/sdk.js')).toEqual({ passed: true });
    expect(asked).toEqual([]);
  });
});

describe('gateUpgrades', () => {
  type Listener = Parameters<UpgradeServer['on']>[1];

  /** A stand-in for Node's HTTP server holding Vite's HMR upgrade listener. */
  function serverWithViteListener() {
    let listeners: Listener[] = [];
    const reachedVite: string[] = [];
    const server: UpgradeServer = {
      listeners: () => [...listeners],
      removeAllListeners: () => (listeners = []),
      on: (_event, listener) => listeners.push(listener),
    };
    server.on('upgrade', (req) => reachedVite.push(req.headers.cookie ?? '(none)'));
    const upgrade = (cookie?: string) =>
      new Promise<string | null>((resolve) => {
        const socket = { end: (data: string) => resolve(data) };
        for (const l of listeners) l({ headers: cookie ? { cookie } : {} }, socket, null);
        setTimeout(() => resolve(null), 0);
      });
    return { server, reachedVite, upgrade };
  }

  it('refuses an upgrade with no paired cookie before Vite ever sees it', async () => {
    const { server, reachedVite, upgrade } = serverWithViteListener();
    gateUpgrades(server, cachedVerdict(async () => false));
    expect(await upgrade()).toMatch(/^HTTP\/1\.1 401/);
    expect(await upgrade('lucidos_device_x=stale')).toMatch(/^HTTP\/1\.1 401/);
    expect(reachedVite).toEqual([]);
  });

  it('hands a paired device upgrade to Vite, so hot reload keeps working', async () => {
    const { server, reachedVite, upgrade } = serverWithViteListener();
    gateUpgrades(server, cachedVerdict(async (c) => c === 'lucidos_device_x=ok'));
    expect(await upgrade('lucidos_device_x=ok')).toBeNull();
    expect(reachedVite).toEqual(['lucidos_device_x=ok']);
  });
});
