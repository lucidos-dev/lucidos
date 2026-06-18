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

/** The workspace id (slug) this bundle is serving, or `null` outside a workspace
 *  (the picker, or a legacy root). */
export const WORKSPACE_ID: string | null =
  IS_PICKER || BASE_PATH === '' ? null : BASE_PATH.slice(1);

/** Prefix a root-relative path (leading `/`) with the base path. `withBase('/sw.js')`
 *  → `/<slug>/sw.js` behind the gateway, `/sw.js` at the root. */
export function withBase(path: string): string {
  const normalized = path.startsWith('/') ? path : `/${path}`;
  return `${BASE_PATH}${normalized}`;
}

/** The PWA / service-worker scope for this context: `/<slug>/`, `/~/`, or `/`.
 *  Always ends with a trailing slash. */
export const SCOPE_PATH: string = `${BASE_PATH}/`;
