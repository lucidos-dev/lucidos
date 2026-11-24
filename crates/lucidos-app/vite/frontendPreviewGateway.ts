
import type { Connect, Plugin } from 'vite';
import { slugifyWorkspaceName } from '../src/utils/slug';

export const PREVIEW_GATEWAY_ORIGIN_ENV = 'LUCIDOS_FRONTEND_PREVIEW_GATEWAY_ORIGIN';
export const PREVIEW_WORKSPACE_ID_ENV = 'LUCIDOS_FRONTEND_PREVIEW_WORKSPACE_ID';
export const PREVIEW_GATEWAY_CA_ENV = 'LUCIDOS_FRONTEND_PREVIEW_GATEWAY_CA';

export const PREVIEW_PROXIED_PREFIXES = ['/api', '/app', '/data'] as const;

export interface PreviewGateway {
  origin: string;
  workspaceId: string;
  caPath: string;
}

export function previewGatewayFromEnv(
  env: Record<string, string | undefined>,
): PreviewGateway | undefined {
  const origin = env[PREVIEW_GATEWAY_ORIGIN_ENV]?.trim();
  const workspaceId = env[PREVIEW_WORKSPACE_ID_ENV]?.trim();
  const caPath = env[PREVIEW_GATEWAY_CA_ENV]?.trim();
  if (!origin || !workspaceId || !caPath) return undefined;
  if (slugifyWorkspaceName(workspaceId) !== workspaceId) return undefined;
  return { origin, workspaceId, caPath };
}

interface PreviewProxyEntry<Agent> {
  target: string;
  changeOrigin: boolean;
  secure: boolean;
  agent: Agent;
}

function proxiedPrefixes(gw: PreviewGateway): string[] {
  return [...PREVIEW_PROXIED_PREFIXES, `/${gw.workspaceId}/`];
}

export function previewProxy<Agent>(
  gw: PreviewGateway,
  agent: Agent,
): Record<string, PreviewProxyEntry<Agent>> {
  const entry = (target: string): PreviewProxyEntry<Agent> => ({
    target,
    changeOrigin: true,
    secure: true,
    agent,
  });
  return {
    ...Object.fromEntries(
      PREVIEW_PROXIED_PREFIXES.map((prefix) => [prefix, entry(`${gw.origin}/${gw.workspaceId}`)]),
    ),
    [`/${gw.workspaceId}/`]: entry(gw.origin),
  };
}

export const PREVIEW_TLS_CERT_ENV = 'LUCIDOS_PREVIEW_TLS_CERT';
export const PREVIEW_TLS_KEY_ENV = 'LUCIDOS_PREVIEW_TLS_KEY';
export const PREVIEW_USER_TLS_CERT_ENV = 'LUCIDOS_PREVIEW_USER_TLS_CERT';
export const PREVIEW_USER_TLS_KEY_ENV = 'LUCIDOS_PREVIEW_USER_TLS_KEY';

export interface TlsPairPaths {
  cert: string;
  key: string;
}

export interface PreviewTls {
  leaf: TlsPairPaths;
  user?: TlsPairPaths;
}

function pairFromEnv(
  env: Record<string, string | undefined>,
  certName: string,
  keyName: string,
): TlsPairPaths | undefined {
  const cert = env[certName]?.trim();
  const key = env[keyName]?.trim();
  return cert && key ? { cert, key } : undefined;
}

export function previewTlsFromEnv(env: Record<string, string | undefined>): PreviewTls | undefined {
  const leaf = pairFromEnv(env, PREVIEW_TLS_CERT_ENV, PREVIEW_TLS_KEY_ENV);
  if (!leaf) return undefined;
  const user = pairFromEnv(env, PREVIEW_USER_TLS_CERT_ENV, PREVIEW_USER_TLS_KEY_ENV);
  return user ? { leaf, user } : { leaf };
}

export function previewBindHost(tls: PreviewTls): true | '127.0.0.1' {
  return tls.user ? true : '127.0.0.1';
}

export function sniPicksLeaf(servername: string): boolean {
  return servername.toLowerCase() === 'localhost';
}

export function isProxiedPath(gw: PreviewGateway, path: string): boolean {
  return proxiedPrefixes(gw).some((prefix) => path.startsWith(prefix));
}

export function sessionAdmits(status: number, body: unknown): boolean {
  if (status !== 200 || typeof body !== 'object' || body === null) return false;
  const session = body as { paired?: unknown; device_id?: unknown; local?: unknown };
  return session.paired === true && typeof session.device_id === 'string' && session.local !== true;
}

const VERDICT_TTL_MS = 10_000;
const VERDICT_CACHE_LIMIT = 64;

const REFUSED_PAGE = `<!doctype html><meta charset="utf-8"><title>Lucidos preview</title>
<p>This frontend preview needs a paired Lucidos device.
Open Lucidos in this browser first, then open the preview again.</p>`;

export type CookieVerdict = (cookie: string | undefined) => Promise<boolean>;

export function cachedVerdict(admits: (cookie: string) => Promise<boolean>): CookieVerdict {
  const verdicts = new Map<string, { ok: boolean; until: number }>();
  return async (cookie) => {
    if (!cookie) return false;
    const cached = verdicts.get(cookie);
    if (cached && cached.until > Date.now()) return cached.ok;
    const ok = await admits(cookie).catch(() => false);
    if (verdicts.size >= VERDICT_CACHE_LIMIT) verdicts.clear();
    verdicts.set(cookie, { ok, until: Date.now() + VERDICT_TTL_MS });
    return ok;
  };
}

export interface GateRequest {
  url?: string;
  headers: { cookie?: string };
}
export interface GateResponse {
  statusCode: number;
  setHeader(name: string, value: string): void;
  end(body: string): void;
}

export function previewAuthMiddleware(
  gw: PreviewGateway,
  verdict: CookieVerdict,
): (req: GateRequest, res: GateResponse, next: () => void) => void {
  return (req, res, next) => {
    if (isProxiedPath(gw, req.url ?? '/')) return next();
    void verdict(req.headers.cookie).then((ok) => {
      if (ok) return next();
      res.statusCode = 401;
      res.setHeader('content-type', 'text/html; charset=utf-8');
      res.end(REFUSED_PAGE);
    });
  };
}

type UpgradeListener = (req: GateRequest, socket: { end(data: string): void }, head: unknown) => void;

export interface UpgradeServer {
  listeners(event: 'upgrade'): UpgradeListener[];
  removeAllListeners(event: 'upgrade'): unknown;
  on(event: 'upgrade', listener: UpgradeListener): unknown;
}

export function gateUpgrades(server: UpgradeServer, verdict: CookieVerdict): void {
  const vites = server.listeners('upgrade');
  server.removeAllListeners('upgrade');
  server.on('upgrade', (req, socket, head) => {
    void verdict(req.headers.cookie).then((ok) => {
      if (!ok) {
        socket.end('HTTP/1.1 401 Unauthorized\r\nConnection: close\r\n\r\n');
        return;
      }
      for (const listener of vites) listener.call(server, req, socket, head);
    });
  });
}

export function previewAuthGate(
  gw: PreviewGateway,
  admits: (cookie: string) => Promise<boolean>,
): Plugin {
  const verdict = cachedVerdict(admits);
  const middleware = previewAuthMiddleware(gw, verdict);
  return {
    name: 'lucidos-frontend-preview-auth-gate',
    apply: 'serve',
    configureServer(server) {
      server.middlewares.use(middleware as unknown as Connect.NextHandleFunction);
      if (server.httpServer) gateUpgrades(server.httpServer as unknown as UpgradeServer, verdict);
    },
  };
}
