import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// The toast dismissal now reads/writes the GLOBAL `preferences` signal (store)
// and persists via the API client, instead of per-device localStorage. Mock both
// so these unit tests neither load the full store graph nor hit HTTP — and so the
// unrelated blocks below (refreshClient, getServedBuildId, …) stay isolated the
// way they were before sw-update started importing the store.
vi.mock('../store/store', async () => {
  const { signal } = await import('@preact/signals');
  return { preferences: signal({ status: 'not-loaded' }), showToast: vi.fn() };
});
vi.mock('../api/client', () => ({ setPreference: vi.fn(() => Promise.resolve({})) }));

import {
  markSwUpdateDismissed, wasSwUpdateDismissed, noteUpdateBuildId,
  markEngineVersionDismissed, wasEngineVersionDismissed, noteAnnouncedEngineVersion,
  scheduleServiceWorkerUpdateChecks, requestServiceWorkerBuildId,
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
    // A genuinely newer build (different id) is NOT dismissed → re-surfaces.
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
    // Global write is exactly (key, id) — the OMITTED 3rd arg (device id) is what
    // makes the dismiss workspace-wide (defers on every device).
    expect(mockSetPreference).toHaveBeenCalledWith(SWITCH_KEY, 'disk-x');
    // Optimistic local update so THIS device suppresses on its next poll without
    // waiting for its own PreferencesChanged round-trip; unrelated keys preserved.
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
    // The version poll passes the CURRENTLY on-disk id; a newer one re-surfaces.
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
    expect(shouldReloadForStaleChunk(1_005_000)).toBe(false); // 5s later — still guarded
  });

  it('allows another reload once the window elapses', () => {
    expect(shouldReloadForStaleChunk(1_000_000)).toBe(true);
    expect(shouldReloadForStaleChunk(1_031_000)).toBe(true); // 31s later — window cleared
  });

  it('returns false (no auto-reload) when sessionStorage is unavailable', () => {
    // Spy on the live global — the object `shouldReloadForStaleChunk` actually
    // calls — not on `Storage.prototype`. The prototype route only worked while
    // the test env happened to hand us a real `Storage` instance; test-setup.ts
    // now always installs a plain in-memory stub (so touching Node's built-in
    // Web Storage can't warn), which no longer inherits from that prototype.
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

describe('scheduleServiceWorkerUpdateChecks', () => {
  const originalNavigator = globalThis.navigator;

  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
    Object.defineProperty(globalThis, 'navigator', { value: originalNavigator, configurable: true });
  });

  function stubNavigator(sw: unknown): void {
    Object.defineProperty(globalThis, 'navigator', {
      value: sw === undefined ? {} : { serviceWorker: sw },
      configurable: true,
    });
  }

  it('calls registration.update() multiple times across the rebuild window', async () => {
    const update = vi.fn(() => Promise.resolve());
    const getRegistration = vi.fn(() => Promise.resolve({ update }));
    stubNavigator({ getRegistration });

    scheduleServiceWorkerUpdateChecks();
    expect(getRegistration).not.toHaveBeenCalled(); // all deferred

    await vi.advanceTimersByTimeAsync(30_000);
    expect(getRegistration.mock.calls.length).toBeGreaterThanOrEqual(4);
    expect(update.mock.calls.length).toBeGreaterThanOrEqual(4);
  });

  it('is a no-op when service workers are unavailable', () => {
    stubNavigator(undefined); // navigator without serviceWorker
    expect(() => {
      scheduleServiceWorkerUpdateChecks();
      vi.advanceTimersByTime(30_000);
    }).not.toThrow();
  });

  it('swallows a failing update() (best-effort, self-recovering)', async () => {
    const getRegistration = vi.fn(() => Promise.reject(new Error('no SW')));
    stubNavigator({ getRegistration });
    scheduleServiceWorkerUpdateChecks();
    await expect(vi.advanceTimersByTimeAsync(30_000)).resolves.not.toThrow();
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
    await Promise.resolve(); // let the ready promise settle
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

  // A refresh is a continuation of the session the user is already in, so the
  // next document must paint the quiet cover instead of replaying the cold-launch
  // brand animation. The flag has to be stamped SYNCHRONOUSLY here: the no-SW
  // branch reloads on the very next line, and the fast path reloads a microtask
  // later, so a deferred write would lose the race with its own reload.
  it('marks the next document as a refresh before any reload can happen', () => {
    const setItem = vi.fn();
    Object.defineProperty(globalThis, 'sessionStorage', {
      value: { setItem, getItem: () => null, removeItem: () => {} },
      configurable: true,
    });
    stubNavigator(undefined);
    refreshClient();
    expect(setItem).toHaveBeenCalledWith('lucidos-splash-quiet', '1');
    // Stamped before the reload, not after it.
    expect(setItem.mock.invocationCallOrder[0]).toBeLessThan(reload.mock.invocationCallOrder[0]);
  });

  // Storage can be refused (private mode / opaque origin). Losing the cover is
  // cosmetic; losing the refresh is not.
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
    // The block must flip synchronously, before any async SW work, so the
    // UiBlockingOverlay dims the screen the instant a refresh is requested.
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
      // A new build is installing — refreshClient must wait for the swap.
      getRegistration: () => Promise.resolve({ update, installing: {} }),
    });

    refreshClient();
    await vi.advanceTimersByTimeAsync(0); // flush getRegistration().then(update)
    expect(update).toHaveBeenCalledTimes(1);
    expect(reload).not.toHaveBeenCalled(); // new worker installing — don't reload stale

    // New worker skipWaiting()s and claims the page.
    for (const fn of listeners['controllerchange'] ?? []) fn();
    expect(reload).toHaveBeenCalledTimes(1);

    // The safety timeout must not fire a second reload (reloaded guard + cleared timer).
    await vi.advanceTimersByTimeAsync(5000);
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it('busts only the shell caches, then reloads, when nothing is swapping', async () => {
    const update = vi.fn(() => Promise.resolve());
    stubNavigator({
      addEventListener: () => {},
      getRegistration: () => Promise.resolve({ update }), // no installing/waiting worker
    });
    const { del } = stubCaches(['lucidos-shell-abc123', 'lucidos-blob-v1', 'unrelated']);

    refreshClient();
    await vi.advanceTimersByTimeAsync(0);

    // Only the shell cache is dropped — the immutable blob cache + anything
    // unrecognized is left intact.
    expect(del).toHaveBeenCalledTimes(1);
    expect(del).toHaveBeenCalledWith('lucidos-shell-abc123');
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it('busts the shell cache then reloads if a detected new worker never claims', async () => {
    const update = vi.fn(() => Promise.resolve());
    stubNavigator({
      addEventListener: () => {}, // controllerchange never fires
      getRegistration: () => Promise.resolve({ update, waiting: {} }),
    });
    const { del } = stubCaches(['lucidos-shell-old']);

    refreshClient();
    await vi.advanceTimersByTimeAsync(0);
    expect(reload).not.toHaveBeenCalled(); // waiting for the swap

    await vi.advanceTimersByTimeAsync(4000); // safety timeout → bust + reload
    expect(del).toHaveBeenCalledWith('lucidos-shell-old');
    expect(reload).toHaveBeenCalledTimes(1);
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
  const originalNavigator = globalThis.navigator;
  const originalFetch = globalThis.fetch;

  function stubNavigator(hasServiceWorker: boolean): void {
    Object.defineProperty(globalThis, 'navigator', {
      value: hasServiceWorker ? { serviceWorker: {} } : {},
      configurable: true,
    });
  }
  function stubFetchSwBody(body: string, ok = true): void {
    globalThis.fetch = vi.fn(() => Promise.resolve({
      ok,
      text: () => Promise.resolve(body),
    })) as unknown as typeof fetch;
  }

  beforeEach(() => stubNavigator(true));
  afterEach(() => {
    Object.defineProperty(globalThis, 'navigator', { value: originalNavigator, configurable: true });
    globalThis.fetch = originalFetch;
  });

  it('returns null when service workers are unavailable', async () => {
    stubNavigator(false);
    stubFetchSwBody("const BUILD_ID = 'server999';");
    expect(await getServedBuildId()).toBe(null);
  });

  it('returns the served build id when present', async () => {
    stubFetchSwBody("const BUILD_ID = '63624440fc8f';");
    expect(await getServedBuildId()).toBe('63624440fc8f');
  });

  it('returns null for the un-stamped placeholder (dev server)', async () => {
    stubFetchSwBody("const BUILD_ID = '__LUCIDOS_BUILD_ID__';");
    expect(await getServedBuildId()).toBe(null);
  });

  it('returns null when sw.js has no BUILD_ID line', async () => {
    stubFetchSwBody('// some unrelated worker without the marker');
    expect(await getServedBuildId()).toBe(null);
  });

  it('returns null on a non-ok response', async () => {
    stubFetchSwBody("const BUILD_ID = '63624440fc8f';", false);
    expect(await getServedBuildId()).toBe(null);
  });

  it('returns null (best-effort) when the fetch rejects', async () => {
    globalThis.fetch = vi.fn(() => Promise.reject(new Error('offline'))) as unknown as typeof fetch;
    expect(await getServedBuildId()).toBe(null);
  });
});
