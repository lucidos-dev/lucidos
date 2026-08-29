import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { json, API, isTransientFetchError, retryTransientRead, throwIfNotOk, ApiError } from './_core';
import { engineRestarting } from '../../store/store';

// While the engine restarts (Apply & Restart) every connection is dropped, so a
// GET fired in that window hits a dead socket and surfaces as
// `TypeError: Load failed` — which the page behind the "Restarting engine…"
// overlay paints as a spurious "Failed to load…" error. `_core` holds GET reads
// until the restart completes; these tests pin that gate, plus its two
// exemptions (the health probe and mutations).

const originalFetch = globalThis.fetch;

function okJson(): Response {
  return new Response(JSON.stringify({ ok: true }), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

describe('read gate during engine restart', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockImplementation(() => Promise.resolve(okJson()));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    engineRestarting.value = false;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    engineRestarting.value = false;
    vi.restoreAllMocks();
  });

  it('holds a GET while restarting, then runs it once the restart completes', async () => {
    engineRestarting.value = true;
    let settled = false;
    const p = json(`${API}/changes`).then(() => { settled = true; });

    // Let microtasks drain — the read must NOT touch the network mid-restart.
    await Promise.resolve();
    await Promise.resolve();
    expect(mockFetch).not.toHaveBeenCalled();
    expect(settled).toBe(false);

    // Watchdog flips the flag on reconnect; the queued read now runs.
    engineRestarting.value = false;
    await p;
    expect(mockFetch).toHaveBeenCalledTimes(1);
    expect(settled).toBe(true);
  });

  it('does NOT gate the health probe — it must run so the watchdog can detect the engine returned', async () => {
    engineRestarting.value = true;
    await json(`${API}/health`);
    expect(mockFetch).toHaveBeenCalledTimes(1);
    expect(mockFetch.mock.calls[0][0]).toContain('/health');
  });

  it('does NOT gate mutations (a non-GET through json) — only reads queue', async () => {
    engineRestarting.value = true;
    await json(`${API}/some/mutation`, { method: 'POST' });
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });

  it('runs GET reads immediately when not restarting', async () => {
    await json(`${API}/changes`);
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });
});

/** WebKit (iOS Safari, the packaged WKWebView) rejects an aborted fetch with its
 *  own `AbortError: Fetch is aborted` instead of the signal's `reason`, so our
 *  fired deadline arrives looking exactly like a page-lifecycle cancel. That is
 *  how a timed-out repositories read painted "Failed to load repositories /
 *  Fetch is aborted" in the packaged app. `_core` re-stamps it as the
 *  `TimeoutError` Chrome and Firefox already deliver. */
describe('client-side deadline normalization', () => {
  /** Rejects the way WebKit does the moment the composed signal aborts. */
  function abortingFetch() {
    return vi.fn((_url: string, init?: RequestInit) => new Promise<Response>((_resolve, reject) => {
      const fail = () => reject(new DOMException('Fetch is aborted', 'AbortError'));
      if (init?.signal?.aborted) fail();
      else init?.signal?.addEventListener('abort', fail);
    }));
  }

  beforeEach(() => { engineRestarting.value = false; });
  afterEach(() => { globalThis.fetch = originalFetch; vi.restoreAllMocks(); });

  it('re-stamps our own fired deadline as TimeoutError, not a bare abort', async () => {
    globalThis.fetch = abortingFetch() as unknown as typeof fetch;
    await expect(json(`${API}/repositories`, undefined, 1))
      .rejects.toMatchObject({ name: 'TimeoutError' });
  });

  it('leaves a caller-initiated abort as AbortError — that cancel was deliberate', async () => {
    globalThis.fetch = abortingFetch() as unknown as typeof fetch;
    const controller = new AbortController();
    const p = json(`${API}/threads/search`, { signal: controller.signal });
    // Let awaitEngineReady + the fetch call drain before cancelling.
    await Promise.resolve();
    await Promise.resolve();
    controller.abort();
    await expect(p).rejects.toMatchObject({ name: 'AbortError' });
  });
});

/**
 * An error body is a payload, and a payload is not a message.
 *
 * The regression (iOS Safari at 390pt): a compose PUT reached the workspace
 * gateway while the engine was away, so the gateway answered its 503 boot
 * splash, an 8 KB HTML page. `throwIfNotOk` put that page into `ApiError.reason`
 * verbatim. The toast rendered it as a bold line over a bulleted list of the
 * page's own `<meta>` tags, covering the transcript.
 *
 * The whole contract is here, because the shapes only differ by body: JSON keeps
 * its field, markup is discarded, plain text is reduced to one clamped line.
 * Reasoning about them apart is how "surface the body, it might be a panic"
 * became "surface the body, it might be a web page".
 */
describe('a failed response never surfaces its body', () => {
  beforeEach(() => { engineRestarting.value = false; });
  afterEach(() => { globalThis.fetch = originalFetch; vi.restoreAllMocks(); });

  /** What `crates/lucidos-gateway/src/proxy.rs::starting_page` actually sends,
   *  down to the marker header the boot splash is identified by. Truncated: the
   *  real page inlines the whole splash stylesheet. */
  function bootSplash(): Response {
    const html = [
      '<!doctype html><html><head>',
      '<meta http-equiv="refresh" content="2">',
      '<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">',
      '<meta name="theme-color" content="#0a4ea8">',
      '</head><body><div class="boot-splash">Starting engine</div></body></html>',
    ].join('\n');
    return new Response(html, {
      status: 503,
      headers: {
        'Content-Type': 'text/html; charset=utf-8',
        'Retry-After': '2',
        'x-lucidos-boot-splash': '1',
      },
    });
  }

  /** The rejection `throwIfNotOk` throws for this response. */
  async function reject(res: Response): Promise<ApiError> {
    return throwIfNotOk(res).then(
      () => { throw new Error('throwIfNotOk resolved on a failed response'); },
      (err: unknown) => err as ApiError,
    );
  }

  it('discards the gateway boot splash and says the workspace is restarting', async () => {
    const err = await reject(bootSplash());

    expect(err.reason).toBe('Lucidos is restarting');
    expect(err.message).not.toMatch(/[<>]/);
    expect(err.message.length).toBeLessThan(60);
    // Nothing to read fields off, so nothing is kept: holding the text is how
    // it reached the screen.
    expect(err.body).toBeUndefined();
    expect(err.bootSplash).toBe(true);
  });

  it('discards an UNMARKED HTML holding page too', async () => {
    // A reverse proxy or a captive portal in front of the gateway sends its own
    // page and knows nothing about our marker.
    const err = await reject(new Response('<html><body><h1>502 Bad Gateway</h1></body></html>', {
      status: 502,
      // Explicit because a constructed Response carries no reason phrase, where
      // an HTTP/1.1 answer does. The HTTP/2 case (always `''`) is below.
      statusText: 'Bad Gateway',
      headers: { 'Content-Type': 'text/html' },
    }));

    expect(err.message).not.toMatch(/[<>]/);
    expect(err.reason).toBe('Bad Gateway');
    // Only a 503 holding page means "not serving yet". A 502 is a real failure.
    expect(err.bootSplash).toBe(false);
  });

  it('keeps a JSON error message exactly as the engine wrote it', async () => {
    const err = await reject(new Response(JSON.stringify({ error: 'thread discarded' }), {
      status: 410,
      headers: { 'Content-Type': 'application/json' },
    }));

    expect(err.message).toBe('410 thread discarded');
    expect(err.body).toEqual({ error: 'thread discarded' });
  });

  it('keeps a JSON body whole, so a caller can format its own card', async () => {
    // The archive endpoint's 409 carries `blocking`, which the caller lists.
    const body = { reason: 'thread has running children', blocking: ['a', 'b'] };
    const err = await reject(new Response(JSON.stringify(body), { status: 409 }));

    expect(err.reason).toBe('thread has running children');
    expect(err.body).toEqual(body);
  });

  it('reduces a plain-text body to its first line, so a panic drops its trace', async () => {
    const err = await reject(new Response(
      'No extraction provider available\n\nstack backtrace:\n   0: rust_begin_unwind\n   1: core::panicking',
      { status: 503, headers: { 'Content-Type': 'text/plain' } },
    ));

    expect(err.reason).toBe('No extraction provider available');
    expect(err.message).not.toContain('\n');
    // The ENGINE answered, so this is a verdict the user is owed, not a
    // holding page.
    expect(err.bootSplash).toBe(false);
  });

  it('clamps a plain-text body that is one very long line', async () => {
    const err = await reject(new Response('x'.repeat(5000), {
      status: 500,
      headers: { 'Content-Type': 'text/plain' },
    }));

    expect(err.reason.length).toBeLessThanOrEqual(200);
    expect(err.reason.endsWith('…')).toBe(true);
  });

  it('names the status when the body is empty and statusText is too', async () => {
    // HTTP/2 carries no reason phrase, so `res.statusText` is always `''` there.
    // A bare "Compose sync failed: 409" tells the user nothing.
    const err = await reject(new Response(null, { status: 409, statusText: '' }));

    expect(err.reason).toBe('HTTP 409');
  });

  it('leaves a body read that FAILED transiently alone', async () => {
    // Reading the body is a second chance to fail, and not every failure is
    // about the content: the deadline is still armed while the stream runs, and
    // a dropped tunnel rejects mid-body. Re-stamping those as unreadable JSON
    // would make a radio handoff a verdict, and the park and retry paths read
    // exactly that classification.
    for (const rejection of [
      new DOMException('Fetch is aborted', 'AbortError'),
      new TypeError('Load failed'),
    ]) {
      const res = new Response('{}', { status: 200 });
      vi.spyOn(res, 'json').mockRejectedValue(rejection);
      globalThis.fetch = vi.fn().mockResolvedValue(res) as unknown as typeof fetch;

      await json(`${API}/changes`).then(
        () => { throw new Error('the failed body read resolved'); },
        (err: unknown) => {
          expect(err).toBe(rejection);
          expect(isTransientFetchError(err)).toBe(true);
        },
      );
    }
  });

  it('does not quote an OK body that is not JSON either', async () => {
    // A captive portal or a tunnel interstitial answers 200 with its login
    // page, so `throwIfNotOk` waves it through and `res.json()` is what fails.
    // V8 puts the payload's first characters in the `SyntaxError` it throws.
    globalThis.fetch = vi.fn().mockResolvedValue(new Response(
      '<!doctype html><html><head><title>Sign in</title></head></html>',
      { status: 200, headers: { 'Content-Type': 'text/html' } },
    )) as unknown as typeof fetch;

    await json(`${API}/changes`).then(
      () => { throw new Error('a page parsed as JSON'); },
      (err: Error) => {
        expect(err.message).not.toMatch(/[<>]/);
        expect(err.message).toBe('The server sent a reply Lucidos could not read');
      },
    );
  });
});

describe('isTransientFetchError / retryTransientRead', () => {
  it('classifies browser-cancelled, timed-out and transport rejections as transient', () => {
    expect(isTransientFetchError(new DOMException('Fetch is aborted', 'AbortError'))).toBe(true);
    expect(isTransientFetchError(new DOMException('timed out', 'TimeoutError'))).toBe(true);
    expect(isTransientFetchError(new TypeError('Load failed'))).toBe(true);
    expect(isTransientFetchError(new TypeError('Failed to fetch'))).toBe(true);
  });

  it('classifies the gateway boot splash as transient: it is no verdict at all', () => {
    // The gateway answered for an engine it could not reach, so the request
    // never got to the thing that decides. Every caller that parks or retries
    // reads this predicate, and a compose draft that takes the verdict branch
    // instead is dropped from the re-send queue.
    expect(isTransientFetchError(new ApiError(503, 'Lucidos is restarting', undefined, true))).toBe(true);
  });

  it('does NOT classify a real backend failure as transient', () => {
    expect(isTransientFetchError(new ApiError(500, 'DB error'))).toBe(false);
    expect(isTransientFetchError(new TypeError('x.map is not a function'))).toBe(false);
    expect(isTransientFetchError(new SyntaxError('Unexpected token <'))).toBe(false);
  });

  it('does NOT classify a 503 the ENGINE sent as transient', () => {
    // "The embedding model is still loading" is an answer the user is owed.
    // Silencing it leaves a rebuild button doing nothing, with no reason given.
    const jsonBody = new ApiError(503, 'embedder not ready', { error: 'embedder not ready' });
    expect(isTransientFetchError(jsonBody)).toBe(false);
    expect(isTransientFetchError(new ApiError(503, 'No extraction provider available'))).toBe(false);
  });

  it('retries a transient rejection once and resolves with the second attempt', async () => {
    const read = vi.fn()
      .mockRejectedValueOnce(new DOMException('Fetch is aborted', 'AbortError'))
      .mockResolvedValueOnce('ok');
    await expect(retryTransientRead(read)).resolves.toBe('ok');
    expect(read).toHaveBeenCalledTimes(2);
  });

  it('rethrows a non-transient rejection without a second call', async () => {
    const read = vi.fn().mockRejectedValue(new ApiError(500, 'DB error'));
    await expect(retryTransientRead(read)).rejects.toThrow('DB error');
    expect(read).toHaveBeenCalledTimes(1);
  });

  it('surfaces the second failure when the retry fails too', async () => {
    const read = vi.fn().mockRejectedValue(new DOMException('timed out', 'TimeoutError'));
    await expect(retryTransientRead(read)).rejects.toMatchObject({ name: 'TimeoutError' });
    expect(read).toHaveBeenCalledTimes(2);
  });
});
