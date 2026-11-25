import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { enginePackaged } from '../store/enginePackaging';
import { perfRecordingOn, trimToCap, recordPerfSample, flushPerfQueue, _resetPerfQueueForTesting, _setPerfEnabledForTesting, isPerfEnabled, setPerfEnabled, clearPerfOptIn, onPerfOptInChange, PERF_FLAG_KEY } from './perfQueue';

describe('perf flag accessors', () => {
  beforeEach(() => { enginePackaged.value = true; });

  afterEach(() => {
    enginePackaged.value = true;
    vi.unstubAllGlobals();
  });

  it('round-trips through localStorage (the single source of truth for the gate)', () => {
    const store: Record<string, string> = {};
    vi.stubGlobal('localStorage', {
      getItem: (k: string) => (k in store ? store[k] : null),
      setItem: (k: string, v: string) => { store[k] = v; },
      removeItem: (k: string) => { delete store[k]; },
    });
    expect(isPerfEnabled()).toBe(false);
    setPerfEnabled(true);
    expect(store[PERF_FLAG_KEY]).toBe('1');
    expect(isPerfEnabled()).toBe(true);
    setPerfEnabled(false);
    expect(PERF_FLAG_KEY in store).toBe(false);
    expect(isPerfEnabled()).toBe(false);
  });

  it('resolves the recording gate for each engine without storing a source opt-in', () => {
    let stored: string | null = null;
    const setItem = vi.fn((_key: string, value: string) => { stored = value; });
    const removeItem = vi.fn(() => { stored = null; });
    vi.stubGlobal('localStorage', { getItem: () => stored, setItem, removeItem });

    expect(perfRecordingOn()).toBe(false);
    enginePackaged.value = false;
    expect(perfRecordingOn()).toBe(true);
    setPerfEnabled(false);
    expect(perfRecordingOn()).toBe(true);
    expect(setItem).not.toHaveBeenCalled();
    expect(removeItem).not.toHaveBeenCalled();

    enginePackaged.value = true;
    expect(perfRecordingOn()).toBe(false);
    setPerfEnabled(true);
    expect(perfRecordingOn()).toBe(true);
    enginePackaged.value = null;
    expect(perfRecordingOn()).toBe(false);
    enginePackaged.value = true;
    expect(perfRecordingOn()).toBe(true);
  });

  it('clears a stored opt-in from source while recording stays on, and tells opt-in listeners', () => {
    const store: Record<string, string> = { [PERF_FLAG_KEY]: '1' };
    vi.stubGlobal('localStorage', {
      getItem: (k: string) => (k in store ? store[k] : null),
      setItem: (k: string, v: string) => { store[k] = v; },
      removeItem: (k: string) => { delete store[k]; },
    });
    const seen: boolean[] = [];
    const unsubscribe = onPerfOptInChange((on) => seen.push(on));
    enginePackaged.value = false;
    expect(isPerfEnabled()).toBe(true);
    expect(perfRecordingOn()).toBe(true);

    clearPerfOptIn();

    expect(PERF_FLAG_KEY in store).toBe(false);
    expect(isPerfEnabled()).toBe(false);
    expect(perfRecordingOn()).toBe(true);
    expect(seen).toEqual([false]);
    clearPerfOptIn();
    expect(seen).toEqual([false]);
    unsubscribe();
  });

  it('never throws and reads false where storage throws', () => {
    vi.stubGlobal('localStorage', {
      getItem: () => { throw new Error('storage blocked'); },
      setItem: () => { throw new Error('storage blocked'); },
      removeItem: () => { throw new Error('storage blocked'); },
    });
    expect(() => setPerfEnabled(true)).not.toThrow();
    expect(() => clearPerfOptIn()).not.toThrow();
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
    _resetPerfQueueForTesting();
    _setPerfEnabledForTesting(true);
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
    vi.stubGlobal('fetch', vi.fn(() => Promise.reject(new Error('engine restarting'))));
    recordPerfSample('open', { eventCount: 1 });
    flushPerfQueue();
    await new Promise((r) => setTimeout(r, 0));
    const ok = vi.fn((_u: string | URL, _i?: RequestInit) => Promise.resolve(new Response(null, { status: 204 })));
    vi.stubGlobal('fetch', ok);
    flushPerfQueue();
    expect(ok).toHaveBeenCalledTimes(1);
    const body = JSON.parse(ok.mock.calls[0][1]!.body as string);
    expect(body).toHaveLength(1);
    expect(body[0]).toMatchObject({ category: 'perf', code: 'open', message: 'open' });
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
    expect(body[0]).toMatchObject({ category: 'perf', code: 'open', message: 'open' });
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
    flushPerfQueue();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('records again once re-enabled (toggle is live)', () => {
    const fetchMock = vi.fn((_url: string | URL, _init?: RequestInit) =>
      Promise.resolve(new Response(null, { status: 204 })));
    vi.stubGlobal('fetch', fetchMock);
    _setPerfEnabledForTesting(false);
    recordPerfSample('open', { eventCount: 1 });
    _setPerfEnabledForTesting(true);
    recordPerfSample('open', { eventCount: 2 });
    flushPerfQueue();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const body = JSON.parse(fetchMock.mock.calls[0][1]!.body as string);
    expect(body).toHaveLength(1);
    expect(body[0]).toMatchObject({ message: 'open', data: { eventCount: 2 } });
  });
});
