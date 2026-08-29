import { effect } from '@preact/signals';
import { engineRestarting } from '../../store/store';
import { BASE_PATH } from '../../utils/basePath';
// A leaf, not devices.ts (which imports json() from here). The gateway control
// client needs the same header and must not pull in the store to get it, so the
// one copy lives in utils/.
import { deviceIdHeader } from '../../utils/deviceIdHeader';
import { clampText } from '../../utils/clampText';

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
     *  back to the raw `httpCode + reason` string.
     *
     *  A non-JSON body is NOT kept here. There is nothing to read fields off,
     *  and holding the raw text is how it reached a toast (`errorReason`). */
    public readonly body?: unknown,
    /** The workspace GATEWAY answered for an engine it could not reach, rather
     *  than the engine answering for itself. Not a verdict about the request,
     *  so `isTransientFetchError` folds it in with the aborts. */
    public readonly bootSplash: boolean = false,
  ) {
    super(`${httpCode} ${reason}`);
    this.name = 'ApiError';
  }
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
 *  - the gateway's boot splash: it answered for an engine it could not reach,
 *    so the request never got to the thing that decides.
 *
 *  Every OTHER `ApiError` is a real verdict and must surface, a 503 the ENGINE
 *  itself sent included: "the embedding model is still loading" is an answer the
 *  user is owed. So is a parse error, and so is any other `TypeError`.
 *
 *  Deliberately WIDER than `isAbortError` (`utils/errorDetail`): the background
 *  paths use that narrower predicate to suppress a cancel while still escalating
 *  a timeout. Here every non-verdict rejection is worth one more attempt. */
export function isTransientFetchError(err: unknown): boolean {
  if (err instanceof ApiError) return err.bootSplash;
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

/** Longest reason taken from a PLAIN-TEXT error body. A sentence, not a
 *  document: the engine's plain-text handlers write one, and a panic writes one
 *  followed by a backtrace this must not follow. */
const MAX_TEXT_REASON_CHARS = 200;

/** What a 503 with nothing to say of its own means to the user. Mid-session is
 *  the only moment a loaded app's fetch reaches the gateway's holding page, and
 *  mid-session the engine was up and went away. */
const RESTARTING_REASON = 'Lucidos is restarting';

/** Is this body markup rather than a message? A holding page from the gateway,
 *  from a reverse proxy, or from a captive portal all arrive this way. */
function looksLikeMarkup(res: Response, text: string): boolean {
  if (/html|xml/i.test(res.headers.get('content-type') ?? '')) return true;
  return text.trimStart().startsWith('<');
}

/** The workspace gateway answered for an engine it could not reach.
 *
 *  `proxy_request` serves its boot splash for EVERY proxied request whose
 *  upstream connect fails, an `/api/v1` mutation included, and marks it with
 *  `x-lucidos-boot-splash` (`crates/lucidos-gateway/src/proxy.rs`). Anything
 *  else in the path can serve its own unmarked HTML holding page, so an HTML
 *  503 counts as well. */
function isBootSplashResponse(res: Response, text: string): boolean {
  if (res.headers.get('x-lucidos-boot-splash') === '1') return true;
  return res.status === 503 && looksLikeMarkup(res, text);
}

/** A short human phrase for a response whose body said nothing usable.
 *
 *  Never `res.statusText` alone: it is `""` over HTTP/2, which leaves a toast
 *  reading "Compose sync failed: 503" and nothing else. */
function statusPhrase(res: Response): string {
  if (res.status === 503) return RESTARTING_REASON;
  return res.statusText || `HTTP ${res.status}`;
}

/** The most specific reason a failed response offers, normalized so nothing a
 *  server sent can paint the screen. `json` is the parsed body, or `undefined`
 *  when the body did not parse.
 *
 *  JSON keeps its `{error}` / `{reason}` field as written: the engine wrote that
 *  for the user. Markup is discarded outright, the gateway's boot splash having
 *  once rendered as a toast listing its own `<meta>` tags. Plain text keeps its
 *  FIRST LINE only, whitespace-collapsed and clamped. It stays a sentence, and
 *  can never grow the bullets `parseToastMessage` reads out of newlines. */
function errorReason(res: Response, text: string, json: unknown): string {
  const obj = json as Record<string, unknown> | null | undefined;
  if (typeof obj?.error === 'string' && obj.error) return obj.error;
  if (typeof obj?.reason === 'string' && obj.reason) return obj.reason;
  if (json === undefined && text && !looksLikeMarkup(res, text)) {
    const firstLine = text.split('\n', 1)[0].replace(/\s+/g, ' ').trim();
    if (firstLine) return clampText(firstLine, MAX_TEXT_REASON_CHARS);
  }
  return statusPhrase(res);
}

/** Throw `ApiError` with the most specific reason the response offers, and
 *  never the raw body. See `errorReason` for what each body shape yields. */
export async function throwIfNotOk(res: Response): Promise<void> {
  if (res.ok) return;
  const text = await res.text().catch(() => '');
  // `undefined` means "did not parse", which a JSON body can never be: `null`
  // parses to `null`. That is what tells `errorReason` the text is unstructured.
  let json: unknown;
  if (text) {
    try {
      json = JSON.parse(text);
    } catch { /* not JSON, and the text is never surfaced */ }
  }
  throw new ApiError(
    res.status,
    errorReason(res, text, json),
    json,
    isBootSplashResponse(res, text),
  );
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
  try {
    return await res.json() as T;
  } catch (err) {
    // A PARSE failure only. Reading the body can also fail for reasons that say
    // nothing about its content. The deadline stays armed while the stream
    // runs, so a slow body past it rejects with `AbortError`. A connection
    // dropped mid-body rejects with a transport `TypeError`. Both are transient
    // and their callers park or retry, so re-stamping either would turn a radio
    // handoff into a verdict.
    if (!(err instanceof SyntaxError)) throw err;
    // The other half of `throwIfNotOk`'s rule, for a body that answered OK and
    // then turned out not to be JSON: a captive portal or a tunnel interstitial
    // sends its login page with a 200. V8 quotes the payload's first characters
    // into the `SyntaxError` it throws, so the raw `<!doctype html` lands in
    // whatever renders the rejection. Say what happened instead.
    throw new SyntaxError('The server sent a reply Lucidos could not read');
  }
}

/** Like `json<T>` but for endpoints that return plain text (file contents,
 *  raw markdown, etc.). Same error / timeout / signal handling. */
export async function text(url: string, init?: RequestInit, timeoutMs = 10000): Promise<string> {
  const res = await fetchWithDefaults(url, init, timeoutMs);
  await throwIfNotOk(res);
  return res.text();
}
