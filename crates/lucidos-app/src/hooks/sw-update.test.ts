import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('../store/store', async () => {
  const { signal } = await import('@preact/signals');
  return { preferences: signal({ status: 'not-loaded' }), showToast: vi.fn() };
});
vi.mock('../api/client', () => ({ setPreference: vi.fn(() => Promise.resolve({})) }));

import {
  markSwUpdateDismissed, wasSwUpdateDismissed, noteUpdateBuildId,
  markEngineVersionDismissed, wasEngineVersionDismissed, noteAnnouncedEngineVersion,
  requestServiceWorkerBuildId,
  refreshClient, clientRefreshing, getServedBuildId, shouldReloadForStaleChunk,
} from './sw-update';
import { preferences } from '../store/store';
import { setPreference } from '../api/client';

const mockSetPreference = vi.mocked(setPreference);
const SWITCH_KEY = 'engine_switch_dismissed_build';
const CLIENT_KEY = 'client_refresh_dismissed_build';

describe('toast dismissal — workspace-global, build-id-keyed (engine switch + client refresh)', () => {
  beforeEach(() => {
    mockSetPreference.mockReset();
    mockSetPreference.mockResolvedValue({} as never);
    preferences.value = { status: 'not-loaded' };
  });

  it('wasEngineVersionDismissed / wasSwUpdateDismissed match only the exact dismissed build id', () => {
    preferences.value = { status: 'loaded', data: { [SWITCH_KEY]: 'disk-a', [CLIENT_KEY]: 'served-a' } };
    expect(wasEngineVersionDismissed('disk-a')).toBe(true);
    expect(wasSwUpdateDismissed('served-a')).toBe(true);
    expect(wasEngineVersionDismissed('disk-b')).toBe(false);
    expect(wasSwUpdateDismissed('served-b')).toBe(false);
  });

  it('fails OPEN — unloaded preferences read as not-dismissed (toast surfaces)', () => {
    preferences.value = { status: 'not-loaded' };
    expect(wasEngineVersionDismissed('disk-a')).toBe(false);
    expect(wasSwUpdateDismissed('served-a')).toBe(false);
  });

  it('markEngineVersionDismissed writes the GLOBAL switch preference (no device id) + optimistic update', () => {
    preferences.value = { status: 'loaded', data: { theme: 'dark' } };
    noteAnnouncedEngineVersion('disk-x');
    markEngineVersionDismissed();
    expect(mockSetPreference).toHaveBeenCalledWith(SWITCH_KEY, 'disk-x');
    expect(preferences.value).toMatchObject({ status: 'loaded', data: { theme: 'dark', [SWITCH_KEY]: 'disk-x' } });
    expect(wasEngineVersionDismissed('disk-x')).toBe(true);
  });

  it('markSwUpdateDismissed writes the GLOBAL client-refresh preference (no device id)', () => {
    preferences.value = { status: 'loaded', data: {} };
    noteUpdateBuildId('served-x');
    markSwUpdateDismissed();
    expect(mockSetPreference).toHaveBeenCalledWith(CLIENT_KEY, 'served-x');
    expect(wasSwUpdateDismissed('served-x')).toBe(true);
  });

  it('a genuinely newer build re-surfaces after an earlier dismiss', () => {
    preferences.value = { status: 'loaded', data: {} };
    noteAnnouncedEngineVersion('disk-a');
    markEngineVersionDismissed();
    expect(wasEngineVersionDismissed('disk-a')).toBe(true);
    expect(wasEngineVersionDismissed('disk-b')).toBe(false);
  });
});

describe('shouldReloadForStaleChunk loop guard', () => {
  beforeEach(() => {
    sessionStorage.clear();
  });

  it('allows the first reload and reserves the window', () => {
    expect(shouldReloadForStaleChunk(1_000_000)).toBe(true);
  });

  it('blocks a second reload inside the window', () => {
    expect(shouldReloadForStaleChunk(1_000_000)).toBe(true);
    expect(shouldReloadForStaleChunk(1_005_000)).toBe(false);
  });

  it('allows another reload once the window elapses', () => {
    expect(shouldReloadForStaleChunk(1_000_000)).toBe(true);
    expect(shouldReloadForStaleChunk(1_031_000)).toBe(true);
  });

  it('returns false (no auto-reload) when sessionStorage is unavailable', () => {
    const spy = vi.spyOn(globalThis.sessionStorage, 'getItem').mockImplementation(() => {
      throw new Error('opaque origin');
    });
    try {
      expect(shouldReloadForStaleChunk(1_000_000)).toBe(false);
    } finally {
      spy.mockRestore();
    }
  });
});

describe('requestServiceWorkerBuildId', () => {
  const originalNavigator = globalThis.navigator;

  afterEach(() => {
    Object.defineProperty(globalThis, 'navigator', { value: originalNavigator, configurable: true });
  });

  function stubNavigator(sw: unknown): void {
    Object.defineProperty(globalThis, 'navigator', {
      value: sw === undefined ? {} : { serviceWorker: sw },
      configurable: true,
    });
  }

  it('posts a get-build-id query to the controller when one exists', () => {
    const postMessage = vi.fn();
    stubNavigator({ controller: { postMessage }, ready: Promise.resolve({}) });
    requestServiceWorkerBuildId();
    expect(postMessage).toHaveBeenCalledWith({ type: 'lucidos:get-build-id' });
  });

  it('falls back to the ready registration\'s active worker when no controller (first load)', async () => {
    const postMessage = vi.fn();
    stubNavigator({ controller: null, ready: Promise.resolve({ active: { postMessage } }) });
    requestServiceWorkerBuildId();
    await Promise.resolve();
    expect(postMessage).toHaveBeenCalledWith({ type: 'lucidos:get-build-id' });
  });

  it('is a no-op when service workers are unavailable', () => {
    stubNavigator(undefined);
    expect(() => requestServiceWorkerBuildId()).not.toThrow();
  });
});

describe('refreshClient', () => {
  const originalNavigator = globalThis.navigator;
  const originalLocation = (globalThis as { location?: unknown }).location;
  const originalCaches = (globalThis as { caches?: unknown }).caches;
  const originalSessionStorage = (globalThis as { sessionStorage?: unknown }).sessionStorage;
  let reload: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.useFakeTimers();
    reload = vi.fn();
    clientRefreshing.value = false;
    Object.defineProperty(globalThis, 'location', { value: { reload }, configurable: true });
  });
  afterEach(() => {
    vi.useRealTimers();
    Object.defineProperty(globalThis, 'navigator', { value: originalNavigator, configurable: true });
    Object.defineProperty(globalThis, 'location', { value: originalLocation, configurable: true });
    if (originalCaches === undefined) {
      delete (globalThis as { caches?: unknown }).caches;
    } else {
      Object.defineProperty(globalThis, 'caches', { value: originalCaches, configurable: true });
    }
    if (originalSessionStorage === undefined) {
      delete (globalThis as { sessionStorage?: unknown }).sessionStorage;
    } else {
      Object.defineProperty(globalThis, 'sessionStorage', {
        value: originalSessionStorage,
        configurable: true,
      });
    }
  });

  function stubNavigator(sw: unknown): void {
    Object.defineProperty(globalThis, 'navigator', {
      value: sw === undefined ? {} : { serviceWorker: sw },
      configurable: true,
    });
  }

  function stubCaches(names: string[]): { del: ReturnType<typeof vi.fn> } {
    const del = vi.fn(() => Promise.resolve(true));
    Object.defineProperty(globalThis, 'caches', {
      value: { keys: () => Promise.resolve(names), delete: del },
      configurable: true,
    });
    return { del };
  }

  it('reloads immediately when service workers are unavailable', () => {
    stubNavigator(undefined);
    refreshClient();
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it('marks the next document as a refresh before any reload can happen', () => {
    const setItem = vi.fn();
    Object.defineProperty(globalThis, 'sessionStorage', {
      value: { setItem, getItem: () => null, removeItem: () => {} },
      configurable: true,
    });
    stubNavigator(undefined);
    refreshClient();
    expect(setItem).toHaveBeenCalledWith('lucidos-splash-quiet', '1');
    expect(setItem.mock.invocationCallOrder[0]).toBeLessThan(reload.mock.invocationCallOrder[0]);
  });

  it('still refreshes when sessionStorage refuses the flag', () => {
    Object.defineProperty(globalThis, 'sessionStorage', {
      value: { setItem: () => { throw new Error('denied'); } },
      configurable: true,
    });
    stubNavigator(undefined);
    expect(() => refreshClient()).not.toThrow();
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it('blocks the UI immediately by raising clientRefreshing', () => {
    stubNavigator({
      addEventListener: () => {},
      getRegistration: () => Promise.resolve({ update: vi.fn(() => Promise.resolve()), installing: {} }),
    });
    expect(clientRefreshing.value).toBe(false);
    refreshClient();
    expect(clientRefreshing.value).toBe(true);
  });

  it('waits for a detected new worker to claim, then reloads on controllerchange', async () => {
    const listeners: Record<string, Function[]> = {};
    const update = vi.fn(() => Promise.resolve());
    stubNavigator({
      addEventListener: (type: string, fn: Function) => { (listeners[type] ??= []).push(fn); },
      getRegistration: () => Promise.resolve({ update, installing: {} }),
    });

    refreshClient();
    await vi.advanceTimersByTimeAsync(0);
    expect(update).toHaveBeenCalledTimes(1);
    expect(reload).not.toHaveBeenCalled();

    for (const fn of listeners['controllerchange'] ?? []) fn();
    expect(reload).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(5000);
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it('busts only the shell caches, then reloads, when nothing is swapping', async () => {
    const update = vi.fn(() => Promise.resolve());
    stubNavigator({
      addEventListener: () => {},
      getRegistration: () => Promise.resolve({ update }),
    });
    const { del } = stubCaches(['lucidos-shell-abc123', 'lucidos-blob-v1', 'unrelated']);

    refreshClient();
    await vi.advanceTimersByTimeAsync(0);

    expect(del).toHaveBeenCalledTimes(1);
    expect(del).toHaveBeenCalledWith('lucidos-shell-abc123');
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it('busts the shell cache then reloads if a detected new worker never claims', async () => {
    const update = vi.fn(() => Promise.resolve());
    stubNavigator({
      addEventListener: () => {},
      getRegistration: () => Promise.resolve({ update, waiting: {} }),
    });
    const { del } = stubCaches(['lucidos-shell-old']);

    refreshClient();
    await vi.advanceTimersByTimeAsync(0);
    expect(reload).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(4000);
    expect(del).toHaveBeenCalledWith('lucidos-shell-old');
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it('busts the shell cache and reloads when the refresh stalls past its deadline', async () => {
    stubNavigator({
      addEventListener: () => {},
      getRegistration: () => new Promise(() => {}),
    });
    const originalFetch = globalThis.fetch;
    globalThis.fetch = vi.fn(() => new Promise(() => {})) as unknown as typeof fetch;
    const { del } = stubCaches(['lucidos-shell-stale']);
    try {
      refreshClient();
      await vi.advanceTimersByTimeAsync(14_000);
      expect(reload).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1_000);
      expect(del).toHaveBeenCalledWith('lucidos-shell-stale');
      expect(reload).toHaveBeenCalledTimes(1);
    } finally {
      globalThis.fetch = originalFetch;
    }
  });

  it('reloads when there is no service worker registration', async () => {
    stubNavigator({
      addEventListener: () => {},
      getRegistration: () => Promise.resolve(null),
    });

    refreshClient();
    await vi.advanceTimersByTimeAsync(0);
    expect(reload).toHaveBeenCalledTimes(1);
  });
});

describe('getServedBuildId', () => {
  const meta = (id: string) => '<meta name="lucidos-build-id" content="' + id + '">';

  function stubResponse(response: Response): void {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(response));
  }

  afterEach(() => vi.unstubAllGlobals());

  it('reads a meta tag split across chunks and cancels the remaining shell', async () => {
    const encoder = new TextEncoder();
    const cancel = vi.fn();
    const shell = meta('63624440fc8f');
    const stream = new ReadableStream({
      start(controller) {
        controller.enqueue(encoder.encode(shell.slice(0, 20)));
        controller.enqueue(encoder.encode(shell.slice(20)));
      },
      cancel,
    });
    stubResponse(new Response(stream));
    expect(await getServedBuildId()).toBe('63624440fc8f');
    expect(cancel).toHaveBeenCalledOnce();
  });

  it('stops at the byte cap', async () => {
    stubResponse(new Response('é'.repeat(4096) + meta('63624440fc8f')));
    expect(await getServedBuildId()).toBe(null);
  });

  it('falls back to text when there is no stream', async () => {
    const response = new Response();
    vi.spyOn(response, 'text').mockResolvedValue(meta('63624440fc8f'));
    stubResponse(response);
    expect(await getServedBuildId()).toBe('63624440fc8f');
  });

  it('returns null for the unstamped placeholder (dev server)', async () => {
    stubResponse(new Response(meta('__LUCIDOS_BUILD_ID__')));
    expect(await getServedBuildId()).toBe(null);
  });

  it('returns null when the shell has no build id', async () => {
    stubResponse(new Response('<html><head></head></html>'));
    expect(await getServedBuildId()).toBe(null);
  });

  it('ignores a fallback shell while the trusted peer client is running', async () => {
    vi.stubGlobal('document', {
      querySelector: (selector: string) => selector === 'meta[name="lucidos-peer-client"]'
        ? { getAttribute: () => 'peer' }
        : null,
    });
    const fallback = '<meta name="lucidos-peer-client-unavailable" content="peer">';
    stubResponse(new Response(fallback + meta('63624440fc8f')));
    expect(await getServedBuildId()).toBe(null);
  });

  it('still checks for installed client updates while the fallback client is running', async () => {
    const fallback = '<meta name="lucidos-peer-client-unavailable" content="peer">';
    stubResponse(new Response(fallback + meta('63624440fc8f')));
    expect(await getServedBuildId()).toBe('63624440fc8f');
  });

  it('returns null for a pairing shell', async () => {
    stubResponse(new Response(meta('63624440fc8f'), { headers: { 'x-lucidos-pairing': '1' } }));
    expect(await getServedBuildId()).toBe(null);
  });

  it('returns null on a non-ok response', async () => {
    stubResponse(new Response(meta('63624440fc8f'), { status: 503 }));
    expect(await getServedBuildId()).toBe(null);
  });

  it('returns null when the fetch rejects', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
    expect(await getServedBuildId()).toBe(null);
  });
});
