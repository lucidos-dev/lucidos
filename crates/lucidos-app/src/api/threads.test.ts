import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { searchThreads, fetchOlderThreads } from './threads';

// Regression guard for `harden-frontend-api-threads-bare-fetch-no-timeout`.
// Before the fix, `searchThreads` and `fetchOlderThreads` used bare `fetch()`
// directly, bypassing the `json()` helper. That meant no 10s `AbortSignal`
// timeout, no `x-lucidos-device-id` header, and `throwIfNotOk`'s JSON-body
// `{error}` parsing was applied by hand without the cross-cutting pipeline.
// Routing both through `json()` reinstates all three.

const originalFetch = globalThis.fetch;

describe('searchThreads goes through the json() helper', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ results: [] }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    );
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    // json() reads the device id off localStorage to stamp the header. The
    // jsdom shim is in place, so just seed a known id.
    localStorage.setItem('lucidos-device-id', 'device-abc');
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    localStorage.removeItem('lucidos-device-id');
    vi.restoreAllMocks();
  });

  it('sends x-lucidos-device-id header (was missing with bare fetch)', async () => {
    await searchThreads('hello');
    const [, init] = mockFetch.mock.calls[0];
    const headers = init?.headers as Record<string, string> | undefined;
    expect(headers).toBeDefined();
    expect(headers!['x-lucidos-device-id']).toBe('device-abc');
  });

  it('attaches an AbortSignal so the request can time out (was untimed with bare fetch)', async () => {
    await searchThreads('hello');
    const [, init] = mockFetch.mock.calls[0];
    expect(init?.signal).toBeDefined();
    expect(init?.signal).toBeInstanceOf(AbortSignal);
  });

  it('composes the caller-supplied signal with the timeout signal', async () => {
    // The json() helper uses AbortSignal.any to OR the caller signal with the
    // timeout signal. Aborting the caller signal must still abort the fetch.
    const controller = new AbortController();
    await searchThreads('hello', controller.signal);
    const [, init] = mockFetch.mock.calls[0];
    const signal = init?.signal as AbortSignal;
    expect(signal).toBeDefined();
    expect(signal.aborted).toBe(false);
    controller.abort();
    // The composed signal must reflect the caller's abort.
    expect(signal.aborted).toBe(true);
  });

  it('surfaces the engine\'s {error} body via throwIfNotOk (bare fetch returned raw text)', async () => {
    mockFetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ error: 'thread not indexable' }), {
        status: 422,
        headers: { 'Content-Type': 'application/json' },
      }),
    );
    await expect(searchThreads('hello')).rejects.toThrow(/thread not indexable/);
  });
});

describe('fetchOlderThreads goes through the json() helper', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({ threads: [], family_threads: [], has_more: false }),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      ),
    );
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    localStorage.setItem('lucidos-device-id', 'device-xyz');
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    localStorage.removeItem('lucidos-device-id');
    vi.restoreAllMocks();
  });

  it('sends x-lucidos-device-id header (was missing with bare fetch)', async () => {
    await fetchOlderThreads('2026-05-23T00:00:00Z');
    const [, init] = mockFetch.mock.calls[0];
    const headers = init?.headers as Record<string, string> | undefined;
    expect(headers).toBeDefined();
    expect(headers!['x-lucidos-device-id']).toBe('device-xyz');
  });

  it('attaches an AbortSignal so the request can time out (was untimed with bare fetch)', async () => {
    await fetchOlderThreads('2026-05-23T00:00:00Z');
    const [, init] = mockFetch.mock.calls[0];
    expect(init?.signal).toBeDefined();
    expect(init?.signal).toBeInstanceOf(AbortSignal);
  });

  it('surfaces the engine\'s {error} body via throwIfNotOk (bare fetch threw on json())', async () => {
    mockFetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ error: 'cursor expired' }), {
        status: 410,
        headers: { 'Content-Type': 'application/json' },
      }),
    );
    await expect(fetchOlderThreads('2026-05-23T00:00:00Z')).rejects.toThrow(/cursor expired/);
  });

  it('builds the URL with before/limit and optional filters', async () => {
    await fetchOlderThreads('2026-05-23T00:00:00Z', 25, ['chat'], ['trig-1'], ['repo-2'], ['app-3']);
    const [url] = mockFetch.mock.calls[0];
    expect(url).toContain('before=2026-05-23');
    expect(url).toContain('limit=25');
    expect(url).toContain('sources=chat');
    expect(url).toContain('trigger_ids=trig-1');
    expect(url).toContain('repo_ids=repo-2');
    expect(url).toContain('app_ids=app-3');
  });

  it('omits app_ids when no apps are selected', async () => {
    await fetchOlderThreads('2026-05-23T00:00:00Z', 15, ['claude_code']);
    const [url] = mockFetch.mock.calls[0];
    expect(url).not.toContain('app_ids');
  });
});
