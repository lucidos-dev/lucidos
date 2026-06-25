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

/** The reserved sigil prefixing all gateway-owned paths (`/~/…`). */
export const SIGIL = '~';

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

/** The workspace id (slug) this bundle is serving, or `null` outside a workspace
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

/** Pure predicate (testable without the DOM): does this served context let the
 *  app build valid workspace URLs? The picker (`/~`, slug null) and a legacy
 *  direct-engine root (`''`, slug null) are always valid. A workspace context is
 *  valid iff its slug is well-formed — that slug IS the string the base-path
 *  (and thus every origin-relative API URL + the SW scope) is built from, so a
 *  malformed one is exactly what yields the WebKit "string did not match the
 *  expected pattern" failures across every fetch + SW registration. */
export function baseContextValidFor(workspaceId: string | null): boolean {
  return workspaceId === null || SLUG_RE.test(workspaceId);
}

/** True when THIS bundle's served base-path context can build valid workspace
 *  URLs. Wraps {@link baseContextValidFor} with the module's `WORKSPACE_ID`.
 *  `main.tsx` bounces an invalid workspace context to the picker rather than
 *  render a broken app. */
export function baseContextIsValid(): boolean {
  return baseContextValidFor(WORKSPACE_ID);
}
