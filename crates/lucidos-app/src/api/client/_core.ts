import { effect } from '@preact/signals';
import { engineRestarting } from '../../store/store';
import { BASE_PATH } from '../../utils/basePath';

// Base-path aware (ADR 0014): behind the workspace gateway the bundle is served
// under `/<slug>/`, so every API URL must carry that prefix. `BASE_PATH` is
// `/<slug>` behind the gateway (read from the stamped `<base href>`) and `''`
// at a legacy root.
export const API_BASE = BASE_PATH;
// Re-exported, not redeclared: `API` is defined next to `BASE_PATH` in
// utils/basePath so leaf modules (utils/clientLog) can build API URLs without
// importing this file, which pulls in the store. Still the one definition.
export { API } from '../../utils/basePath';

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

/** True for a rejection that says nothing about the request itself, so the
 *  right response is to try again rather than to park a `Loadable` on `failed`:
 *
 *  - `AbortError` — the browser cancelled the fetch (an iOS PWA freezing
 *    mid-flight, a page-lifecycle transition in the packaged WKWebView).
 *  - `TimeoutError` — our own client-side deadline fired (a read issued while
 *    the engine is still booting is the common one).
 *  - a transport `TypeError` — stale connection (see `isTransportError`).
 *
 *  A `4xx`/`5xx` (`ApiError`), a parse error, or any other `TypeError` is a real
 *  verdict and must surface.
 *
 *  Deliberately WIDER than `isAbortError` (`utils/errorDetail`): the background
 *  paths use that narrower predicate to suppress a cancel while still escalating
 *  a timeout. Here every non-verdict rejection is worth one more attempt. */
export function isTransientFetchError(err: unknown): boolean {
  if (err instanceof DOMException) {
    return err.name === 'AbortError' || err.name === 'TimeoutError';
  }
  return isTransportError(err);
}

/** Run an idempotent read once more when the first attempt fails transiently.
 *  For `Loadable`-backed loaders, where a single cancelled fetch would otherwise
 *  stick as a visible failure until something else happens to refetch. The
 *  second failure (or any non-transient one) propagates unchanged.
 *
 *  Only for reads with no caller-supplied `AbortSignal` — a deliberate cancel
 *  would be retried with the same, already-aborted signal. */
export async function retryTransientRead<T>(read: () => Promise<T>): Promise<T> {
  try {
    return await read();
  } catch (err) {
    if (!isTransientFetchError(err)) throw err;
    return read();
  }
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
 *  `mutatingFetch`) and the health probe are exempt.
 *
 *  The signal composition + `TimeoutError` re-stamp below are mirrored by
 *  `restampDeadline` in `packages/lucidos-sdk/src/_fetch.ts`, which covers every
 *  call routed through the SDK (preference writes among them). Two copies on
 *  purpose: the SDK bundles standalone for app iframes and cannot import from
 *  the host, and only this half gates on `awaitEngineReady`. Change one, change
 *  the other. */
async function fetchWithDefaults(url: string, init: RequestInit | undefined, timeoutMs: number): Promise<Response> {
  const method = (init?.method ?? 'GET').toUpperCase();
  if (method === 'GET' && !isHealthProbe(url)) await awaitEngineReady();
  const headers = { ...deviceIdHeader(), ...(init?.headers as Record<string, string> | undefined) };
  const timeoutSignal = AbortSignal.timeout(timeoutMs);
  const signal = init?.signal ? AbortSignal.any([init.signal, timeoutSignal]) : timeoutSignal;
  try {
    return await fetch(url, { ...init, headers, signal });
  } catch (err) {
    // WebKit rejects an aborted fetch with its own `AbortError: Fetch is
    // aborted` rather than the signal's `reason`, so on iOS Safari and in the
    // packaged WKWebView a fired deadline is indistinguishable from a
    // page-lifecycle cancel: it reads as "request cancelled" and the background
    // paths that suppress an AbortError swallow it. Re-stamp it as the
    // TimeoutError Chrome and Firefox already deliver, so the deadline means the
    // same thing on every engine. A caller's own abort wins when both fired —
    // that one was deliberate.
    if (err instanceof DOMException && err.name === 'AbortError'
      && timeoutSignal.aborted && !init?.signal?.aborted) {
      throw new DOMException(`Request timed out after ${timeoutMs}ms`, 'TimeoutError');
    }
    throw err;
  }
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
