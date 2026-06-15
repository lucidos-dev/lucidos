import { effect } from '@preact/signals';
import { engineRestarting } from '../../store/store';

export const API_BASE = '';
// Single source of truth for the HTTP API prefix. Every fetch in this file
// builds onto `${API}/…` so the version is stamped exactly once.
// See CLAUDE.md "API URL Conventions".
export const API = `${API_BASE}/api/v1`;

export class ApiError extends Error {
  constructor(
    public readonly httpCode: number,
    public readonly reason: string,
    /** Parsed JSON body when the engine returned one. Lets callers format
     *  domain-specific error toasts from structured fields (e.g. the archive
     *  endpoint's `{reason, blocking: [...]}` 409 body) instead of falling
     *  back to the raw `httpCode + reason` string. */
    public readonly body?: unknown,
  ) {
    super(`${httpCode} ${reason}`);
    this.name = 'ApiError';
  }
}

// Inlined to avoid a circular import with devices.ts (which imports json()).
function deviceIdHeader(): Record<string, string> {
  if (typeof localStorage === 'undefined') return {};
  const id = localStorage.getItem('lucidos-device-id');
  return id ? { 'x-lucidos-device-id': id } : {};
}

/** The engine drops every connection while it restarts (Apply & Restart). A
 *  GET issued in that window hits the dead socket and surfaces as
 *  `TypeError: Load failed`, flipping whatever `Loadable` it feeds to `failed`
 *  — so the page sitting behind the "Restarting engine…" overlay paints a
 *  spurious "Failed to load…" error (and `loadMoreChanges`, infinite-scroll
 *  observers, and resume refetches all fire blind). Hold reads until the
 *  restart completes instead: the connection watchdog flips `engineRestarting`
 *  back to false on reconnect (or the 300s safety timeout), and the queued read
 *  then runs against the live engine — resolving normally with no error and no
 *  manual refresh. The health probe MUST bypass this (see `fetchWithDefaults`):
 *  it is the very signal the watchdog polls to notice the engine came back, so
 *  gating it would deadlock the gate. */
function awaitEngineReady(): Promise<void> {
  if (!engineRestarting.value) return Promise.resolve();
  return new Promise<void>((resolve) => {
    const dispose = effect(() => {
      if (!engineRestarting.value) {
        dispose();
        resolve();
      }
    });
  });
}

/** True for the `/api/v1/health` probe — the one read that must never be gated
 *  by `awaitEngineReady`, because it's how the connection watchdog detects the
 *  engine is back. */
function isHealthProbe(url: string): boolean {
  return url.endsWith('/health');
}

/** `fetch` wrapped to always send `x-lucidos-device-id` so the engine can
 *  attribute the call to this device. Use when you need the raw `Response`
 *  (custom `AbortSignal`, non-JSON body, status-specific handling like 409).
 *  Pair with `throwIfNotOk(res)` so error responses surface the engine's
 *  `{error}` body. JSON-response endpoints should use `json()`, which adds
 *  the header and parses for you. */
export async function mutatingFetch(url: string, init?: RequestInit): Promise<Response> {
  const headers = { ...deviceIdHeader(), ...(init?.headers as Record<string, string> | undefined) };
  return fetch(url, { ...init, headers });
}

/** Match the `TypeError` messages browsers throw for transport-layer fetch
 *  failures: Safari ("Load failed"), Chrome ("Failed to fetch"), Firefox
 *  ("NetworkError when attempting to fetch resource"). Anything else is a
 *  real bug and must surface, not be silently retried. */
export function isTransportError(err: unknown): boolean {
  return err instanceof TypeError
    && /Load failed|Failed to fetch|NetworkError/i.test(err.message);
}

/** Same as `mutatingFetch` but retries once on a transport-layer error
 *  (iOS Safari surfaces stale-connection failures as `TypeError("Load failed")`
 *  after the PWA backgrounds). Use only for endpoints whose backend handler is
 *  idempotent — a retry must be safe to observe a side-effect twice. The
 *  service worker has the equivalent retry for GETs (`fetchWithRetry` in
 *  sw.js); POSTs bypass the SW because iOS WebKit can't reliably clone
 *  request bodies, so the retry has to live here. */
export async function mutatingFetchIdempotent(url: string, init?: RequestInit): Promise<Response> {
  try {
    return await mutatingFetch(url, init);
  } catch (err) {
    if (isTransportError(err)) return mutatingFetch(url, init);
    throw err;
  }
}

/** Throw `ApiError` with the most specific reason available: the body's
 *  `{error}` field when the body is JSON, the raw text when the body is
 *  non-JSON (so proxy 502 HTML and plain-text panics surface their content),
 *  else `res.statusText`. */
export async function throwIfNotOk(res: Response): Promise<void> {
  if (res.ok) return;
  const text = await res.text().catch(() => '');
  let reason = res.statusText;
  let body: unknown;
  if (text) {
    try {
      body = JSON.parse(text);
      const obj = body as Record<string, unknown> | null;
      if (typeof obj?.error === 'string') reason = obj.error;
      else if (typeof obj?.reason === 'string') reason = obj.reason;
    } catch { reason = text; }
  }
  throw new ApiError(res.status, reason, body);
}

/** AbortSignal.timeout fires with a TimeoutError DOMException, so errorDetail
 *  can distinguish it from a manual AbortError. When the caller supplies its
 *  own `init.signal` (e.g. for cancellable searches), it's composed with the
 *  timeout signal via AbortSignal.any so either path can abort the fetch.
 *
 *  GET reads are held by `awaitEngineReady` while the engine is mid-restart so
 *  they don't paint spurious "Failed to load…" errors on the page behind the
 *  restart overlay; mutations (which never reach here — they go through
 *  `mutatingFetch`) and the health probe are exempt. */
async function fetchWithDefaults(url: string, init: RequestInit | undefined, timeoutMs: number): Promise<Response> {
  const method = (init?.method ?? 'GET').toUpperCase();
  if (method === 'GET' && !isHealthProbe(url)) await awaitEngineReady();
  const headers = { ...deviceIdHeader(), ...(init?.headers as Record<string, string> | undefined) };
  const timeoutSignal = AbortSignal.timeout(timeoutMs);
  const signal = init?.signal ? AbortSignal.any([init.signal, timeoutSignal]) : timeoutSignal;
  return fetch(url, { ...init, headers, signal });
}

export async function json<T>(url: string, init?: RequestInit, timeoutMs = 10000): Promise<T> {
  const res = await fetchWithDefaults(url, init, timeoutMs);
  await throwIfNotOk(res);
  return res.json();
}

/** Like `json<T>` but for endpoints that return plain text (file contents,
 *  raw markdown, etc.). Same error / timeout / signal handling. */
export async function text(url: string, init?: RequestInit, timeoutMs = 10000): Promise<string> {
  const res = await fetchWithDefaults(url, init, timeoutMs);
  await throwIfNotOk(res);
  return res.text();
}
