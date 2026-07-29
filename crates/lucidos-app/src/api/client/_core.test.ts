import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { json, API, isTransientFetchError, retryTransientRead, ApiError } from './_core';
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

describe('isTransientFetchError / retryTransientRead', () => {
  it('classifies browser-cancelled, timed-out and transport rejections as transient', () => {
    expect(isTransientFetchError(new DOMException('Fetch is aborted', 'AbortError'))).toBe(true);
    expect(isTransientFetchError(new DOMException('timed out', 'TimeoutError'))).toBe(true);
    expect(isTransientFetchError(new TypeError('Load failed'))).toBe(true);
    expect(isTransientFetchError(new TypeError('Failed to fetch'))).toBe(true);
  });

  it('does NOT classify a real backend failure as transient', () => {
    expect(isTransientFetchError(new ApiError(500, 'DB error'))).toBe(false);
    expect(isTransientFetchError(new TypeError('x.map is not a function'))).toBe(false);
    expect(isTransientFetchError(new SyntaxError('Unexpected token <'))).toBe(false);
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
