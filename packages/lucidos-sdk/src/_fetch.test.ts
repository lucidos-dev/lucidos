import { describe, it, expect, vi, afterEach } from 'vitest';
import { request, restampDeadline, SdkError } from './_fetch';

/** WebKit (iOS Safari, the packaged WKWebView) rejects an aborted fetch with its
 *  own `AbortError: Fetch is aborted` instead of the signal's `reason`, so a
 *  fired deadline arrives looking exactly like a page-lifecycle cancel. Anything
 *  formatting the error then reports "request cancelled" for what was really a
 *  timeout, which is how a suspended iOS PWA's preference write got mislabeled.
 *  `_fetch` re-stamps it as the `TimeoutError` Chrome and Firefox already
 *  deliver. Mirrors `api/client/_core.test.ts` § "client-side deadline
 *  normalization", the host-side copy of the same rule. */

const originalFetch = globalThis.fetch;

/** Rejects the way WebKit does the moment the composed signal aborts. */
function abortingFetch() {
  return vi.fn((_url: string, init?: RequestInit) => new Promise<Response>((_resolve, reject) => {
    const fail = () => reject(new DOMException('Fetch is aborted', 'AbortError'));
    if (init?.signal?.aborted) fail();
    else init?.signal?.addEventListener('abort', fail);
  }));
}

afterEach(() => {
  globalThis.fetch = originalFetch;
  vi.restoreAllMocks();
});

describe('restampDeadline', () => {
  const fired = (): AbortSignal => AbortSignal.abort();
  const pending = (): AbortSignal => new AbortController().signal;

  it('re-stamps a WebKit generic abort as TimeoutError when our deadline fired', () => {
    const out = restampDeadline(new DOMException('Fetch is aborted', 'AbortError'), fired(), 10000);
    expect(out).toBeInstanceOf(DOMException);
    expect((out as DOMException).name).toBe('TimeoutError');
    expect((out as DOMException).message).toContain('10000ms');
  });

  it('leaves the abort alone when the CALLER also aborted, that cancel was deliberate', () => {
    const out = restampDeadline(
      new DOMException('Fetch is aborted', 'AbortError'),
      fired(),
      10000,
      AbortSignal.abort(),
    );
    expect((out as DOMException).name).toBe('AbortError');
  });

  it('leaves the abort alone when our deadline never fired', () => {
    const out = restampDeadline(new DOMException('Fetch is aborted', 'AbortError'), pending(), 10000);
    expect((out as DOMException).name).toBe('AbortError');
  });

  it('passes a non-abort rejection through untouched', () => {
    const err = new TypeError('Load failed');
    expect(restampDeadline(err, fired(), 10000)).toBe(err);
  });
});

/** The workspace address is the whole reason `apiUrl` is public: an app that
 *  builds an engine URL by hand gets a 404 that a `catch` turns into silence.
 *  Both derivations are locked here because they are what the SDK reads INSTEAD
 *  of the two shapes an author reaches for (a relative path against
 *  `document.baseURI`, or a root-absolute `/api/v1/…`). See
 *  `system-knowhow/js-sdk.md` § lucidos.apiUrl. */
describe('apiUrl workspace-address derivation', () => {
  const originalQuerySelector = globalThis.document.querySelector;
  const originalLocation = (globalThis as unknown as { location?: unknown }).location;

  /** Re-import `_fetch` so its load-time `computeBaseUrl()` reads the DOM we
   *  just installed. The base is resolved once per module instance, so a fresh
   *  instance per case is the only way to exercise both branches. */
  async function apiUrlUnder(
    base: string | null,
    pathname: string,
  ): Promise<(suffix: string) => string> {
    globalThis.document.querySelector = ((): Element | null =>
      (base === null ? null : { getAttribute: () => base } as unknown as Element)) as
      typeof document.querySelector;
    (globalThis as unknown as { location: { pathname: string } }).location = { pathname };
    vi.resetModules();
    return (await import('./_fetch')).apiUrl;
  }

  afterEach(() => {
    globalThis.document.querySelector = originalQuerySelector;
    (globalThis as unknown as { location?: unknown }).location = originalLocation;
  });

  it('reads the SPA shell\'s <base href>, whatever the slug is called', async () => {
    const apiUrl = await apiUrlUnder('/dev/', '/dev/');
    expect(apiUrl('/events/query?limit=1')).toBe('/dev/api/v1/events/query?limit=1');
  });

  it('takes everything before /app/ when there is no <base>, as in an app iframe', async () => {
    const apiUrl = await apiUrlUnder(null, '/dev/app/habit-tracker/');
    expect(apiUrl('/events/query?limit=1')).toBe('/dev/api/v1/events/query?limit=1');
  });

  it('falls back to an unprefixed URL with no <base> and no /app/ segment', async () => {
    const apiUrl = await apiUrlUnder(null, '/');
    expect(apiUrl('/events/query')).toBe('/api/v1/events/query');
  });
});

describe('rawFetch deadline + caller signal', () => {
  it('reports a fired deadline as TimeoutError, never a bare cancel', async () => {
    globalThis.fetch = abortingFetch() as unknown as typeof fetch;
    await expect(request('/preferences', { method: 'PUT' }, 1))
      .rejects.toMatchObject({ name: 'TimeoutError' });
  });

  // Regression: `rawFetch` used to set `signal: controller.signal` on the spread
  // init, which silently DROPPED a caller-supplied signal, so the caller's abort
  // did nothing at all.
  it('honours a caller-supplied signal and keeps its abort an AbortError', async () => {
    globalThis.fetch = abortingFetch() as unknown as typeof fetch;
    const controller = new AbortController();
    const p = request('/threads/list', { signal: controller.signal });
    await Promise.resolve();
    controller.abort();
    await expect(p).rejects.toMatchObject({ name: 'AbortError' });
  });

  it('still throws SdkError with the body reason on a non-ok response', async () => {
    globalThis.fetch = vi.fn(async () => new Response(
      JSON.stringify({ error: 'unknown preference key' }),
      { status: 400, headers: { 'Content-Type': 'application/json' } },
    )) as unknown as typeof fetch;
    await expect(request('/preferences', { method: 'PUT' }))
      .rejects.toThrow(SdkError);
    await expect(request('/preferences', { method: 'PUT' }))
      .rejects.toMatchObject({ httpCode: 400, reason: 'unknown preference key' });
  });
});
