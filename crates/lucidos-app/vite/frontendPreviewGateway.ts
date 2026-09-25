/**
 * The frontend preview's side of the gateway (`crates/lucidos-engine/src/engine/frontend_preview.rs`).
 *
 * The preview is a `vite serve` the engine supervises, rooted in a coding-agent
 * worktree and listening on its OWN port. Vite forwards the engine-owned path
 * prefixes to the workspace's gateway, so the page is same-origin with its own
 * API and needs no preview-awareness: with no `<base href>` stamped it builds
 * `/api/v1/…`, which lands here.
 *
 * **Everything the preview serves needs a paired device (ADR 0267).** The
 * gateway checks the device cookie on each forwarded call. Vite's own files
 * (the bundle, `/@fs/…`, the HMR socket) never reach the gateway, so
 * `previewAuthGate` asks the gateway about the same cookie first. Only the browser's `Cookie` header
 * travels, never the machine-local token: that token would make every LAN
 * caller a local process.
 *
 * **Inert unless the engine asked for it.** `vite.config.ts`'s `server` block
 * also serves a manual `npm run dev`, so with the engine's env vars absent
 * there is no proxy key and no gate at all.
 */

import type { Connect, Plugin } from 'vite';

/** Env vars the engine sets when it spawns the preview. Mirrored in Rust as
 *  `frontend_preview::PREVIEW_GATEWAY_ORIGIN_ENV` / `PREVIEW_WORKSPACE_ID_ENV`. */
export const PREVIEW_GATEWAY_ORIGIN_ENV = 'LUCIDOS_FRONTEND_PREVIEW_GATEWAY_ORIGIN';
export const PREVIEW_WORKSPACE_ID_ENV = 'LUCIDOS_FRONTEND_PREVIEW_WORKSPACE_ID';

/**
 * The engine-owned path prefixes, and why each one has to be forwarded:
 *   `/api`   every HTTP call and the SSE stream (`/api/v1/…`).
 *   `/app`   app-UI pages, loaded into iframes by relative src.
 *   `/data`  the workspace's `data/` tree: artifacts, app assets, images.
 * Everything else is the bundle itself, which Vite serves.
 */
export const PREVIEW_PROXIED_PREFIXES = ['/api', '/app', '/data'] as const;

/** Mirrors the gateway's slug rule, so a junk value never becomes a path. */
const SLUG_SHAPE = /^[a-z0-9][a-z0-9-]*$/;

export interface PreviewGateway {
  /** `{scheme}://127.0.0.1:{port}`, the gateway on loopback. */
  origin: string;
  workspaceId: string;
}

/** The gateway the engine named, or `undefined` when this is not a preview. */
export function previewGatewayFromEnv(
  env: Record<string, string | undefined>,
): PreviewGateway | undefined {
  const origin = env[PREVIEW_GATEWAY_ORIGIN_ENV]?.trim();
  const workspaceId = env[PREVIEW_WORKSPACE_ID_ENV]?.trim();
  if (!origin || !workspaceId || !SLUG_SHAPE.test(workspaceId)) return undefined;
  return { origin, workspaceId };
}

interface PreviewProxyEntry {
  target: string;
  changeOrigin: boolean;
  /** The dev gateway serves its own self-signed cert; the same
   *  `danger_accept_invalid_certs` allowance every intra-host Lucidos hop makes. */
  secure: boolean;
}

/** The request paths `previewProxy` forwards, as Vite matches them: by prefix. */
function proxiedPrefixes(gw: PreviewGateway): string[] {
  return [...PREVIEW_PROXIED_PREFIXES, `/${gw.workspaceId}/`];
}

/**
 * The `server.proxy` map. The bundle's own prefixes go to `/<slug>/…`, since
 * http-proxy prepends the target's path. `/<slug>/` passes through unchanged:
 * an app frame's HTML comes back rescoped to `/<slug>/…`, and the gateway
 * checks the frame's capability against that slug.
 */
export function previewProxy(gw: PreviewGateway): Record<string, PreviewProxyEntry> {
  const entry = (target: string): PreviewProxyEntry => ({ target, changeOrigin: true, secure: false });
  return {
    ...Object.fromEntries(
      PREVIEW_PROXIED_PREFIXES.map((prefix) => [prefix, entry(`${gw.origin}/${gw.workspaceId}`)]),
    ),
    [`/${gw.workspaceId}/`]: entry(gw.origin),
  };
}

/** Does a request path go to the gateway, which authenticates it itself? */
export function isProxiedPath(gw: PreviewGateway, path: string): boolean {
  return proxiedPrefixes(gw).some((prefix) => path.startsWith(prefix));
}

/**
 * The gate's verdict on the gateway's `/~/api/v1/auth/session` answer. Only a
 * paired device passes. Anything else refuses, including an answer that
 * could not be read: unknown is never a yes.
 */
export function sessionAdmits(status: number, body: unknown): boolean {
  if (status !== 200 || typeof body !== 'object' || body === null) return false;
  const session = body as { paired?: unknown; device_id?: unknown; local?: unknown };
  return session.paired === true && typeof session.device_id === 'string' && session.local !== true;
}

/** How long a cookie's verdict is reused. Short, so a revoked device loses the
 *  preview within seconds, and long enough that one page load asks once. */
const VERDICT_TTL_MS = 10_000;
const VERDICT_CACHE_LIMIT = 64;

const REFUSED_PAGE = `<!doctype html><meta charset="utf-8"><title>Lucidos preview</title>
<p>This frontend preview needs a paired Lucidos device.
Open Lucidos in this browser first, then open the preview again.</p>`;

/** Whether a request's `Cookie` header belongs to a paired device. */
export type CookieVerdict = (cookie: string | undefined) => Promise<boolean>;

/**
 * Wrap `admits` (which asks the gateway, `gatewaySession.ts`) with a short
 * per-cookie cache. No cookie, or a failed ask, is a refusal.
 */
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

/** The parts of a Connect request and response the gate touches. */
export interface GateRequest {
  url?: string;
  headers: { cookie?: string };
}
export interface GateResponse {
  statusCode: number;
  setHeader(name: string, value: string): void;
  end(body: string): void;
}

/** Gate every HTTP request Vite answers itself on the gateway's device cookie. */
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

/** The parts of Node's HTTP server that `gateUpgrades` touches. */
export interface UpgradeServer {
  listeners(event: 'upgrade'): UpgradeListener[];
  removeAllListeners(event: 'upgrade'): unknown;
  on(event: 'upgrade', listener: UpgradeListener): unknown;
}

/**
 * Put Vite's own WebSocket upgrade listeners behind the same cookie check.
 *
 * Upgrades never pass through Connect middleware. Vite checks its HMR token
 * only when a handshake carries an `Origin`. Without this gate, a non-browser
 * client on the LAN gets hot-update and error payloads, with source paths and
 * snippets. A browser sends its cookie on the handshake, so hot reload keeps
 * working for a paired device.
 */
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

/**
 * The gate as a plugin. Registered in `configureServer` without a returned
 * function, so it runs before Vite's own middlewares, the proxy included. By
 * then Vite has attached its HMR upgrade listener, which `gateUpgrades` wraps.
 */
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
      // Connect's and Node's own types reach `http`, which this project does
      // not type. The `Gate*` / `UpgradeServer` shapes are the fields used.
      server.middlewares.use(middleware as unknown as Connect.NextHandleFunction);
      if (server.httpServer) gateUpgrades(server.httpServer as unknown as UpgradeServer, verdict);
    },
  };
}
