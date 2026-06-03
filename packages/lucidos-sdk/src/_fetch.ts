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
 */

let _baseUrl = '';
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

export class SdkError extends Error {
  constructor(
    public readonly httpCode: number,
    public readonly reason: string,
  ) {
    super(`${httpCode} ${reason}`);
    this.name = 'SdkError';
  }
}

async function rawFetch(
  path: string,
  init?: RequestInit,
  timeoutMs = 10000,
): Promise<Response> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  const headers: Record<string, string> = {};
  if (_authToken) headers['Authorization'] = `Bearer ${_authToken}`;

  const normalized = path.startsWith('/') ? path : `/${path}`;
  try {
    const res = await fetch(`${_baseUrl}${API_V1}${normalized}`, {
      ...init,
      signal: controller.signal,
      headers: { ...headers, ...(init?.headers as Record<string, string>) },
    });
    if (!res.ok) {
      let reason = res.statusText;
      try {
        const body = await res.json();
        if (body?.error) reason = body.error;
      } catch { /* body not JSON */ }
      throw new SdkError(res.status, reason);
    }
    return res;
  } finally {
    clearTimeout(timeout);
  }
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
