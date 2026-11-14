/**
 * Base-path awareness for the workspace gateway (ADR 0014).
 *
 * One bundle serves under several prefixes:
 *   • `/<slug>/` — a workspace, proxied through the gateway (the engine stamps
 *     `<base href="/<slug>/">` into the served `index.html`).
 *   • `/~/`      — the gateway's picker context (the gateway stamps
 *     `<base href="/~/">`); `~` is the reserved sigil that can never be a slug.
 *   • `/`        — a legacy direct engine (`LUCIDOS_NO_GATEWAY`), base `/`.
 *
 * The prefix is read from that stamped `<base href>` rather than parsed from the
 * pathname, so it is **slug-agnostic** — no hardcoded URL shape, any workspace
 * name works. Every absolute URL the app builds (API calls, the SSE
 * EventSource, the service-worker registration + scope, asset links) carries the
 * prefix so the gateway routes it to the right engine (it strips the prefix
 * before proxying).
 */

import {
  REMOTE_GATEWAY_NAME_MAX_LEN,
  REMOTE_ROUTE_SEPARATOR,
  SIGIL,
} from '../generated/gateway-constants';

/** The literal `<base href>` the server stamped, or `'/'` when absent (no DOM /
 *  unit tests / a server that didn't stamp one). */
function baseHref(): string {
  if (typeof document === 'undefined') return '/';
  const href = document.querySelector('base')?.getAttribute('href');
  return href && href.length > 0 ? href : '/';
}

/** Normalize a `<base href>` to a path prefix with NO trailing slash, `''` at
 *  the root. Tolerates an absolute URL value (`https://h/dev/` → `/dev`). */
export function normalizeBasePath(href: string): string {
  let path = href;
  try {
    if (/^https?:\/\//i.test(href)) path = new URL(href).pathname;
  } catch {
    /* keep the raw value */
  }
  if (!path.startsWith('/')) path = `/${path}`;
  path = path.replace(/\/+$/, ''); // strip trailing slash(es)
  return path; // '' at root, '/<slug>' or '/~' otherwise
}

/** The prefix this bundle is served under: `/<slug>` for a workspace, `/~` for
 *  the picker, `''` at a legacy root. Computed once at module load. */
export const BASE_PATH: string = normalizeBasePath(baseHref());

/** Single source of truth for the HTTP API prefix — every fetch builds onto
 *  `${API}/…` so the version is stamped exactly once. See CLAUDE.md "API URL
 *  Conventions". It lives here, next to `BASE_PATH`, rather than in
 *  `api/client/_core` so that leaf modules can reach the API without pulling in
 *  the store (`_core` imports `engineRestarting`); `_core` re-exports it, so
 *  `import { API } from './client'` keeps working everywhere. */
export const API = `${BASE_PATH}/api/v1`;

/** True when this bundle is the gateway's picker context (`<base href="/~/">`).
 *  `main.tsx` renders the picker iff this is set. */
export const IS_PICKER: boolean = BASE_PATH === `/${SIGIL}`;

/** Read the gateway's external port from the `<meta name="lucidos-gateway-port">`
 *  the engine stamps into the served shell when it runs behind a gateway (ADR
 *  0014). `null` when absent (legacy no-gateway engine, or no DOM in tests) or
 *  malformed — never a `NaN`/garbage port that would build a broken URL. */
function readGatewayPort(): number | null {
  if (typeof document === 'undefined') return null;
  const raw = document
    .querySelector('meta[name="lucidos-gateway-port"]')
    ?.getAttribute('content');
  if (!raw) return null;
  const n = Number(raw);
  return Number.isInteger(n) && n > 0 ? n : null;
}

/** The workspace gateway's port, or `null` when this engine runs with no gateway
 *  (so there is no picker to address). Computed once at module load — used to
 *  build an absolute URL to the gateway picker from a *direct* engine-port page,
 *  whose origin is the engine, not the gateway. */
export const GATEWAY_PORT: number | null = readGatewayPort();

/** Pure: the href for the "Manage workspaces" link, or `null` when no gateway
 *  picker is reachable. Behind the gateway (or in the picker context) the picker
 *  is same-origin, so the relative `/~/?pick` route works. On a direct
 *  engine-port page the gateway is a *different* origin (its own port), so build
 *  an absolute URL from the stamped gateway port and the host the user is already
 *  on (covers localhost AND Tailscale; `protocol` covers http/https). With no
 *  stamped port and not behind a gateway, there is no picker → `null`. */
export function computeGatewayPickerHref(opts: {
  behindGateway: boolean;
  gatewayPort: number | null;
  protocol: string;
  hostname: string;
}): string | null {
  if (opts.behindGateway) return `/${SIGIL}/?pick`;
  if (opts.gatewayPort === null) return null;
  return `${opts.protocol}//${opts.hostname}:${opts.gatewayPort}/${SIGIL}/?pick`;
}

/** The workspace route segment this bundle is serving (`<slug>`, or
 *  `<slug>@<gateway>` for a remote workspace), or `null` outside a workspace
 *  (the picker, or a legacy root). */
export const WORKSPACE_ID: string | null =
  IS_PICKER || BASE_PATH === '' ? null : BASE_PATH.slice(1);

/** Live-wired {@link computeGatewayPickerHref} for the current served context. */
export function gatewayPickerHref(): string | null {
  return computeGatewayPickerHref({
    // Served under `/<slug>/` or the picker `/~/` → this origin IS the gateway.
    behindGateway: WORKSPACE_ID !== null || IS_PICKER,
    gatewayPort: GATEWAY_PORT,
    protocol: typeof location !== 'undefined' ? location.protocol : 'https:',
    hostname: typeof location !== 'undefined' ? location.hostname : 'localhost',
  });
}

/** Prefix a root-relative path (leading `/`) with the base path. `withBase('/sw.js')`
 *  → `/<slug>/sw.js` behind the gateway, `/sw.js` at the root. */
export function withBase(path: string): string {
  const normalized = path.startsWith('/') ? path : `/${path}`;
  return `${BASE_PATH}${normalized}`;
}

/** The PWA / service-worker scope for this context: `/<slug>/`, `/~/`, or `/`.
 *  Always ends with a trailing slash. */
export const SCOPE_PATH: string = `${BASE_PATH}/`;

/** A workspace slug the gateway would accept. Mirrors `registry::slugify`
 *  output (lowercase alphanumerics joined by single hyphens, no leading/trailing
 *  hyphen) — the same shape the picker's `slugifyWorkspaceName` produces. */
const SLUG_RE = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

/** Is `segment` a workspace slug the gateway would serve? */
export function isWorkspaceSlug(segment: string): boolean {
  return SLUG_RE.test(segment);
}

/** A remote gateway's short name: `registry::is_valid_id` capped at
 *  `REMOTE_GATEWAY_NAME_MAX_LEN`, the rule `remote_gateways::valid_name` applies. */
function isRemoteGatewayName(name: string): boolean {
  return (
    name.length <= REMOTE_GATEWAY_NAME_MAX_LEN && /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(name)
  );
}

/** Where a workspace lives, read from its route segment: `<slug>` on this
 *  gateway, or `<slug>@<gateway>` on a remote gateway (ADR 0591). */
export interface WorkspaceRoute {
  slug: string;
  /** The remote gateway's short name, or null for this gateway's own. */
  gateway: string | null;
}

/** Read a route segment, or `null` for one the gateway would not route.
 *  Mirrors `relay::remote_route` for the remote form. */
export function parseWorkspaceRoute(segment: string): WorkspaceRoute | null {
  const at = segment.indexOf(REMOTE_ROUTE_SEPARATOR);
  if (at < 0) return isWorkspaceSlug(segment) ? { slug: segment, gateway: null } : null;
  const slug = segment.slice(0, at);
  const gateway = segment.slice(at + 1);
  return isWorkspaceSlug(slug) && isRemoteGatewayName(gateway) ? { slug, gateway } : null;
}

/** The route segment for `route`, the inverse of {@link parseWorkspaceRoute}. */
export function workspaceRouteSegment(route: WorkspaceRoute): string {
  return route.gateway === null ? route.slug : `${route.slug}${REMOTE_ROUTE_SEPARATOR}${route.gateway}`;
}

/** `id` as a URL path segment. Escapes like `encodeURIComponent`, except the
 *  route separator: the gateway reads the raw path, so `%40` would miss a
 *  remote route. */
export function workspaceRouteUrlSegment(id: string): string {
  return encodeURIComponent(id)
    .split(encodeURIComponent(REMOTE_ROUTE_SEPARATOR))
    .join(REMOTE_ROUTE_SEPARATOR);
}

/** The remote gateway this page's workspace lives on, or null when it is this
 *  gateway's own (or no workspace at all). An unqualified workspace name in a
 *  thread link resolves against it. */
export const REMOTE_GATEWAY: string | null =
  WORKSPACE_ID === null ? null : (parseWorkspaceRoute(WORKSPACE_ID)?.gateway ?? null);

/** Pure predicate (testable without the DOM): does this served context let the
 *  app build valid workspace URLs? The picker (`/~`, slug null) and a legacy
 *  direct-engine root (`''`, slug null) are always valid. A workspace context is
 *  valid iff its route segment is well-formed. That segment IS the string the
 *  base path, every origin-relative API URL and the SW scope are built from. A
 *  malformed one yields WebKit's "string did not match the expected pattern"
 *  on every fetch and SW registration. */
export function baseContextValidFor(workspaceId: string | null): boolean {
  return workspaceId === null || parseWorkspaceRoute(workspaceId) !== null;
}

/** True when THIS bundle's served base-path context can build valid workspace
 *  URLs. Wraps {@link baseContextValidFor} with the module's `WORKSPACE_ID`.
 *  `main.tsx` bounces an invalid workspace context to the picker rather than
 *  render a broken app. */
export function baseContextIsValid(): boolean {
  return baseContextValidFor(WORKSPACE_ID);
}
