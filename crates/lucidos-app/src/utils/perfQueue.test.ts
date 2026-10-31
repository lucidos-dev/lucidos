import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { trimToCap, recordPerfSample, flushPerfQueue, _resetPerfQueueForTesting, _setPerfEnabledForTesting, isPerfEnabled, setPerfEnabled, PERF_FLAG_KEY } from './perfQueue';

describe('perf flag accessors', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('round-trips through localStorage (the single source of truth for the gate)', () => {
    const store: Record<string, string> = {};
    vi.stubGlobal('localStorage', {
      getItem: (k: string) => (k in store ? store[k] : null),
      setItem: (k: string, v: string) => { store[k] = v; },
      removeItem: (k: string) => { delete store[k]; },
    });
    expect(isPerfEnabled()).toBe(false); // default off (absent key)
    setPerfEnabled(true);
    expect(store[PERF_FLAG_KEY]).toBe('1');
    expect(isPerfEnabled()).toBe(true);
    setPerfEnabled(false); // off removes the key, not sets '0'
    expect(PERF_FLAG_KEY in store).toBe(false);
    expect(isPerfEnabled()).toBe(false);
  });

  it('never throws and reads false where storage throws', () => {
    vi.stubGlobal('localStorage', {
      getItem: () => { throw new Error('storage blocked'); },
      setItem: () => { throw new Error('storage blocked'); },
      removeItem: () => { throw new Error('storage blocked'); },
    });
    expect(() => setPerfEnabled(true)).not.toThrow();
    expect(isPerfEnabled()).toBe(false);
  });
});

describe('trimToCap', () => {
  it('returns the buffer unchanged when at or under the cap', () => {
    expect(trimToCap([1, 2, 3], 5)).toEqual([1, 2, 3]);
    expect(trimToCap([1, 2, 3], 3)).toEqual([1, 2, 3]);
  });
  it('drops the OLDEST entries beyond the cap', () => {
    expect(trimToCap([1, 2, 3, 4, 5], 3)).toEqual([3, 4, 5]);
  });
});

describe('perf queue fire-and-forget', () => {
  beforeEach(() => {
    _resetPerfQueueForTesting(); // a re-buffered failed flush must not leak across tests
    _setPerfEnabledForTesting(true); // recording is default-off; these tests exercise the on path
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    _setPerfEnabledForTesting(null);
    _resetPerfQueueForTesting();
  });

  it('recordPerfSample never throws even if fetch is missing', () => {
    vi.stubGlobal('fetch', undefined);
    expect(() => recordPerfSample('open', { eventCount: 10 })).not.toThrow();
  });

  it('flushPerfQueue swallows a rejecting fetch (telemetry never breaks the app)', () => {
    vi.stubGlobal('fetch', vi.fn(() => Promise.reject(new Error('engine down'))));
    recordPerfSample('open', { eventCount: 1 });
    expect(() => flushPerfQueue()).not.toThrow();
  });

  it('re-buffers samples after a network failure so a later flush retries them', async () => {
    // First flush: network rejects → sample is re-buffered, not lost.
    vi.stubGlobal('fetch', vi.fn(() => Promise.reject(new Error('engine restarting'))));
    recordPerfSample('open', { eventCount: 1 });
    flushPerfQueue();
    await new Promise((r) => setTimeout(r, 0)); // drain microtasks so the reject's .catch runs requeueFailed
    // Second flush: engine back up → the re-buffered sample is delivered.
    const ok = vi.fn((_u: string | URL, _i?: RequestInit) => Promise.resolve(new Response(null, { status: 204 })));
    vi.stubGlobal('fetch', ok);
    flushPerfQueue();
    expect(ok).toHaveBeenCalledTimes(1);
    const body = JSON.parse(ok.mock.calls[0][1]!.body as string);
    expect(body).toHaveLength(1);
    expect(body[0]).toMatchObject({ category: 'perf', message: 'open' });
  });

  it('flush batches buffered samples into one client-logs request and clears the buffer', () => {
    const fetchMock = vi.fn((_url: string | URL, _init?: RequestInit) =>
      Promise.resolve(new Response(null, { status: 204 })));
    vi.stubGlobal('fetch', fetchMock);
    recordPerfSample('open', { a: 1 });
    recordPerfSample('answer', { b: 2 });
    flushPerfQueue();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0];
    expect(String(url)).toContain('/internal/client-logs');
    const body = JSON.parse(init!.body as string);
    expect(body).toHaveLength(2);
    expect(body[0]).toMatchObject({ category: 'perf', message: 'open' });
    // Buffer cleared → a second flush sends nothing.
    flushPerfQueue();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});

describe('perf queue default-off gate', () => {
  beforeEach(() => _resetPerfQueueForTesting());
  afterEach(() => {
    vi.unstubAllGlobals();
    _setPerfEnabledForTesting(null);
    _resetPerfQueueForTesting();
  });

  it('records nothing when disabled — no buffer, no flush, no network', () => {
    _setPerfEnabledForTesting(false);
    const fetchMock = vi.fn((_url: string | URL, _init?: RequestInit) =>
      Promise.resolve(new Response(null, { status: 204 })));
    vi.stubGlobal('fetch', fetchMock);
    recordPerfSample('open', { eventCount: 1 });
    recordPerfSample('answer', { eventCount: 2 });
    flushPerfQueue(); // nothing buffered → no request
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('records again once re-enabled (toggle is live)', () => {
    const fetchMock = vi.fn((_url: string | URL, _init?: RequestInit) =>
      Promise.resolve(new Response(null, { status: 204 })));
    vi.stubGlobal('fetch', fetchMock);
    _setPerfEnabledForTesting(false);
    recordPerfSample('open', { eventCount: 1 }); // dropped
    _setPerfEnabledForTesting(true);
    recordPerfSample('open', { eventCount: 2 }); // recorded
    flushPerfQueue();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const body = JSON.parse(fetchMock.mock.calls[0][1]!.body as string);
    expect(body).toHaveLength(1);
    expect(body[0]).toMatchObject({ message: 'open', data: { eventCount: 2 } });
  });
});
