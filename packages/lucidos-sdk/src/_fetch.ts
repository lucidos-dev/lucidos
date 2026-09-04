/**
 * Internal fetch wrapper. Single point for:
 * - Base URL resolution
 * - Auto-prefixing the engine's `/api/v1` HTTP surface
 * - Future auth headers (lucidos.configure({ token }))
 * - Timeout handling
 * - Consistent error shape
 *
 * Public-surface files (notifications.ts, threads.ts, …) pass the path
 * *suffix only* — e.g. `request('/threads/list')`. The `/api/v1` prefix is
 * stamped here so individual SDK files can't drift. Files that need to build
 * a URL for the browser (e.g. `lucidos.data.url(path)`, `sse.ts`) call
 * `apiUrl(suffix)` for the same auto-prefixing.
 *
 * `_storage.ts` imports `getBaseUrl` from here, so the two form a cycle. It is
 * safe: both sides export hoisted function declarations, and neither calls the
 * other while its module is evaluating.
 */

import { wsDeviceId } from './_storage';

/** Derive the workspace base path (`/<slug>`) the SDK runs under, so calls to
 *  the engine's `/api/v1` surface carry the gateway prefix (ADR 0014). Two
 *  contexts, both slug-agnostic:
 *   • The main app loads the SPA shell, which the engine stamps with
 *     `<base href="/<slug>/">` — read that (authoritative, any slug name).
 *   • An app iframe loads at `/<slug>/app/<app_id>/…` with no `<base>` — derive
 *     the prefix as everything before `/app/`.
 *  Falls back to `''` (legacy root / no DOM). `configure({ baseUrl })` overrides
 *  for embedders that set it explicitly. */
function computeBaseUrl(): string {
  if (typeof document !== 'undefined') {
    const href = document.querySelector('base')?.getAttribute('href');
    if (href) {
      let path = href;
      try {
        if (/^https?:\/\//i.test(href)) path = new URL(href).pathname;
      } catch {
        /* keep raw */
      }
      return path.replace(/\/+$/, ''); // '' at root, '/<slug>' or '/~' otherwise
    }
  }
  // App iframe (no <base>): the prefix is everything before `/app/`.
  const path = (typeof window !== 'undefined' && window.location && window.location.pathname) || '';
  const i = path.indexOf('/app/');
  return i >= 0 ? path.slice(0, i) : '';
}

let _baseUrl = computeBaseUrl();
let _authToken: string | undefined;

/** Sole hard-coded reference to the API version. Every other file in the SDK
 *  routes through `request*` (which prepends this) or `apiUrl` (same). */
const API_V1 = '/api/v1';

export function configure(opts: { baseUrl?: string; token?: string }) {
  if (opts.baseUrl !== undefined) _baseUrl = opts.baseUrl;
  if (opts.token !== undefined) _authToken = opts.token;
}

export function getBaseUrl(): string {
  return _baseUrl;
}

/** Resolve an `/api/v1`-relative suffix to an absolute URL (for `EventSource`,
 *  `<script src>`, anchor hrefs, etc.). Pass the path *after* `/api/v1`, e.g.
 *  `apiUrl('/events')` → `<baseUrl>/api/v1/events`. */
export function apiUrl(suffix: string): string {
  const normalized = suffix.startsWith('/') ? suffix : `/${suffix}`;
  return `${_baseUrl}${API_V1}${normalized}`;
}

/** The versioned API root, with NO trailing slash, for a caller that builds
 *  several sibling URLs from one base. Prefer `apiUrl` for a single path:
 *  `apiUrl('')` is not this, it yields a trailing slash. */
export function apiBase(): string {
  return `${_baseUrl}${API_V1}`;
}

export class SdkError extends Error {
  constructor(
    public readonly httpCode: number,
    public readonly reason: string,
  ) {
    super(`${httpCode} ${reason}`);
    this.name = 'SdkError';
  }
}

/** WebKit rejects an aborted fetch with its own `AbortError: Fetch is aborted`
 *  rather than the signal's `reason`, so on iOS Safari and in the packaged
 *  WKWebView a fired deadline is indistinguishable from a page-lifecycle cancel:
 *  it reads as "request cancelled" to anything formatting the error. Re-stamp it
 *  as the `TimeoutError` Chrome and Firefox already deliver, so the deadline
 *  means the same thing on every engine. A caller's own abort wins when both
 *  fired, because that one was deliberate.
 *
 *  Mirrors `fetchWithDefaults` in `crates/lucidos-app/src/api/client/_core.ts`.
 *  Deliberately a second copy rather than a shared import: the SDK bundles
 *  standalone for app iframes and cannot depend on the host app, and the host
 *  half additionally gates GETs on `awaitEngineReady`. Change one, change the
 *  other. */
export function restampDeadline(
  err: unknown,
  timeoutSignal: AbortSignal,
  timeoutMs: number,
  callerSignal?: AbortSignal | null,
): unknown {
  if (err instanceof DOMException && err.name === 'AbortError'
    && timeoutSignal.aborted && !callerSignal?.aborted) {
    return new DOMException(`Request timed out after ${timeoutMs}ms`, 'TimeoutError');
  }
  return err;
}

/** The header the engine resolves a request's actor from (`api::actor`). */
const DEVICE_ID_HEADER = 'x-lucidos-device-id';

async function rawFetch(
  path: string,
  init?: RequestInit,
  timeoutMs = 10000,
): Promise<Response> {
  const headers: Record<string, string> = {};
  // Every app call says which device it came from, so a publish or a trigger
  // edit is attributed to the person who clicked it. Without it the engine has
  // no evidence of who is calling and refuses the write (ADR 0169).
  const deviceId = wsDeviceId();
  if (deviceId) headers[DEVICE_ID_HEADER] = deviceId;
  if (_authToken) headers['Authorization'] = `Bearer ${_authToken}`;

  const normalized = path.startsWith('/') ? path : `/${path}`;
  // `AbortSignal.timeout` rejects with a `TimeoutError`, which callers can tell
  // apart from a deliberate `AbortError` cancel; the old manual AbortController
  // could only ever produce the latter. A caller's `init.signal` is COMPOSED
  // with the deadline rather than overwritten (the manual controller silently
  // dropped it), so either can abort the request.
  const timeoutSignal = AbortSignal.timeout(timeoutMs);
  const signal = init?.signal ? AbortSignal.any([init.signal, timeoutSignal]) : timeoutSignal;

  let res: Response;
  try {
    res = await fetch(`${_baseUrl}${API_V1}${normalized}`, {
      ...init,
      signal,
      headers: { ...headers, ...(init?.headers as Record<string, string>) },
    });
  } catch (err) {
    throw restampDeadline(err, timeoutSignal, timeoutMs, init?.signal);
  }
  if (!res.ok) {
    let reason = res.statusText;
    try {
      const body = await res.json();
      if (body?.error) reason = body.error;
    } catch { /* body not JSON */ }
    throw new SdkError(res.status, reason);
  }
  return res;
}

export async function request<T>(
  path: string,
  init?: RequestInit,
  timeoutMs = 10000,
): Promise<T> {
  const res = await rawFetch(path, init, timeoutMs);
  const text = await res.text();
  if (!text) return null as T;
  return JSON.parse(text);
}

export async function requestText(
  path: string,
  init?: RequestInit,
  timeoutMs = 10000,
): Promise<string> {
  const res = await rawFetch(path, init, timeoutMs);
  return res.text();
}

export async function requestVoid(
  path: string,
  init?: RequestInit,
  timeoutMs = 10000,
): Promise<void> {
  await rawFetch(path, init, timeoutMs);
}
