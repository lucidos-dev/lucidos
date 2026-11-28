import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { json, mutatingFetch, API, isTransientFetchError, retryTransient, throwIfNotOk, ApiError } from './_core';
import { trackDeviceRegistration } from '../../utils/deviceRegistration';
import { engineRestarting } from '../../store/store';


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

    await Promise.resolve();
    await Promise.resolve();
    expect(mockFetch).not.toHaveBeenCalled();
    expect(settled).toBe(false);

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

describe('client-side deadline normalization', () => {
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
    await Promise.resolve();
    await Promise.resolve();
    controller.abort();
    await expect(p).rejects.toMatchObject({ name: 'AbortError' });
  });
});

describe('a failed response never surfaces its body', () => {
  beforeEach(() => { engineRestarting.value = false; });
  afterEach(() => { globalThis.fetch = originalFetch; vi.restoreAllMocks(); });

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
    expect(err.body).toBeUndefined();
    expect(err.bootSplash).toBe(true);
  });

  it('discards an UNMARKED HTML holding page too', async () => {
    const err = await reject(new Response('<html><body><h1>502 Bad Gateway</h1></body></html>', {
      status: 502,
      statusText: 'Bad Gateway',
      headers: { 'Content-Type': 'text/html' },
    }));

    expect(err.message).not.toMatch(/[<>]/);
    expect(err.reason).toBe('Bad Gateway');
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
    const err = await reject(new Response(null, { status: 409, statusText: '' }));

    expect(err.reason).toBe('HTTP 409');
  });

  it('leaves a body read that FAILED transiently alone', async () => {
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

describe('isTransientFetchError / retryTransient', () => {
  it('classifies browser-cancelled, timed-out and transport rejections as transient', () => {
    expect(isTransientFetchError(new DOMException('Fetch is aborted', 'AbortError'))).toBe(true);
    expect(isTransientFetchError(new DOMException('timed out', 'TimeoutError'))).toBe(true);
    expect(isTransientFetchError(new TypeError('Load failed'))).toBe(true);
    expect(isTransientFetchError(new TypeError('Failed to fetch'))).toBe(true);
  });

  it('classifies the gateway boot splash as transient: it is no verdict at all', () => {
    expect(isTransientFetchError(new ApiError(503, 'Lucidos is restarting', undefined, true))).toBe(true);
  });

  it('does NOT classify a real backend failure as transient', () => {
    expect(isTransientFetchError(new ApiError(500, 'DB error'))).toBe(false);
    expect(isTransientFetchError(new TypeError('x.map is not a function'))).toBe(false);
    expect(isTransientFetchError(new SyntaxError('Unexpected token <'))).toBe(false);
  });

  it('does NOT classify a 503 the ENGINE sent as transient', () => {
    const jsonBody = new ApiError(503, 'embedder not ready', { error: 'embedder not ready' });
    expect(isTransientFetchError(jsonBody)).toBe(false);
    expect(isTransientFetchError(new ApiError(503, 'No extraction provider available'))).toBe(false);
  });

  it('retries a transient rejection once and resolves with the second attempt', async () => {
    const read = vi.fn()
      .mockRejectedValueOnce(new DOMException('Fetch is aborted', 'AbortError'))
      .mockResolvedValueOnce('ok');
    await expect(retryTransient(read)).resolves.toBe('ok');
    expect(read).toHaveBeenCalledTimes(2);
  });

  it('rethrows a non-transient rejection without a second call', async () => {
    const read = vi.fn().mockRejectedValue(new ApiError(500, 'DB error'));
    await expect(retryTransient(read)).rejects.toThrow('DB error');
    expect(read).toHaveBeenCalledTimes(1);
  });

  it('surfaces the second failure when the retry fails too', async () => {
    const read = vi.fn().mockRejectedValue(new DOMException('timed out', 'TimeoutError'));
    await expect(retryTransient(read)).rejects.toMatchObject({ name: 'TimeoutError' });
    expect(read).toHaveBeenCalledTimes(2);
  });
});

describe('a mutation waits for a pending device registration', () => {
  let mockFetch: ReturnType<typeof vi.fn>;
  let finishRegistration: () => void;

  beforeEach(() => {
    mockFetch = vi.fn().mockImplementation(() => Promise.resolve(okJson()));
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    trackDeviceRegistration(new Promise<void>((resolve) => { finishRegistration = resolve; }));
  });

  afterEach(async () => {
    finishRegistration();
    await Promise.resolve();
    globalThis.fetch = originalFetch;
  });

  it('holds a mutatingFetch POST until registration settles', async () => {
    const sent = mutatingFetch(`${API}/threads`, { method: 'POST', body: '{}' });
    await Promise.resolve();
    expect(mockFetch).not.toHaveBeenCalled();

    finishRegistration();
    await sent;
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });

  it('holds a json() PUT until registration settles', async () => {
    const sent = json(`${API}/compose/x`, { method: 'PUT', body: '{}' });
    await Promise.resolve();
    expect(mockFetch).not.toHaveBeenCalled();

    finishRegistration();
    await sent;
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });

  it('sends a mutation at once when registration has settled', async () => {
    finishRegistration();
    await Promise.resolve();
    await Promise.resolve();
    const sent = mutatingFetch(`${API}/threads`, { method: 'POST', body: '{}' });
    expect(mockFetch).toHaveBeenCalledTimes(1);
    await sent;
  });

  it('lets a read through without waiting', async () => {
    await json(`${API}/threads/list`);
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });
});

describe('API read transport retry', () => {
  afterEach(() => { globalThis.fetch = originalFetch; });
  it('retries a transport failure through json', async () => {
    const request = vi.fn().mockRejectedValueOnce(new TypeError('Load failed')).mockResolvedValueOnce(okJson());
    globalThis.fetch = request;
    await expect(json(API + '/threads/list')).resolves.toEqual({ ok: true });
    expect(request).toHaveBeenCalledTimes(2);
  });
  it('does not retry the latency health probe', async () => {
    const request = vi.fn().mockRejectedValue(new TypeError('Load failed'));
    globalThis.fetch = request;
    await expect(json(API + '/health')).rejects.toThrow('Load failed');
    expect(request).toHaveBeenCalledTimes(1);
  });
  it('does not retry body parsing', async () => {
    const request = vi.fn().mockResolvedValue(new Response('bad JSON'));
    globalThis.fetch = request;
    await expect(json(API + '/threads/list')).rejects.toThrow();
    expect(request).toHaveBeenCalledTimes(1);
  });
});
