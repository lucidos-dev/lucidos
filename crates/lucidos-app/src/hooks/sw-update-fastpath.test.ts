import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// Pin the running bundle's build id so the fast path can match (or miss) the
// served sw.js BUILD_ID deterministically — in a real build the
// `lucidos-sw-stamp` plugin replaces the `__LUCIDOS_BUILD_ID__` placeholder with
// this exact value. Must be hoisted above the import of the module under test.
vi.mock('virtual:build-id', () => ({ CLIENT_BUILD_ID: 'build-current' }));

import {
  refreshClient,
  isRunningServedBuild,
  reloadForStaleChunk,
  SHELL_CACHE_PREFIX,
} from './sw-update';

const CURRENT_SHELL_CACHE = `${SHELL_CACHE_PREFIX}build-current`;

describe('isRunningServedBuild', () => {
  it('true when the served build id equals the running bundle id', () => {
    expect(isRunningServedBuild('build-current')).toBe(true);
  });

  it('false when the served build id differs (a newer build is live)', () => {
    expect(isRunningServedBuild('build-next')).toBe(false);
  });

  it('false when the served build id is unknown (offline / dev / fetch failure)', () => {
    expect(isRunningServedBuild(null)).toBe(false);
  });
});

describe('refreshClient — build-id fast path', () => {
  const originalNavigator = globalThis.navigator;
  const originalLocation = (globalThis as { location?: unknown }).location;
  const originalCaches = (globalThis as { caches?: unknown }).caches;
  const originalFetch = globalThis.fetch;
  let reload: ReturnType<typeof vi.fn>;
  let getRegistration: ReturnType<typeof vi.fn>;
  let cacheDelete: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.useFakeTimers();
    reload = vi.fn();
    Object.defineProperty(globalThis, 'location', { value: { reload }, configurable: true });

    // A registration with no installing/waiting worker — the "nothing swapping"
    // branch, so a fall-through to the dance ends in bust + reload.
    getRegistration = vi.fn(() => Promise.resolve({ update: vi.fn(() => Promise.resolve()) }));
    Object.defineProperty(globalThis, 'navigator', {
      value: { serviceWorker: { addEventListener: () => {}, getRegistration } },
      configurable: true,
    });

    cacheDelete = vi.fn(() => Promise.resolve(true));
    Object.defineProperty(globalThis, 'caches', {
      value: { keys: () => Promise.resolve([CURRENT_SHELL_CACHE]), delete: cacheDelete },
      configurable: true,
    });
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
    globalThis.fetch = originalFetch;
  });

  function stubServedBuildId(id: string): void {
    globalThis.fetch = vi.fn(() => Promise.resolve({
      ok: true,
      text: () => Promise.resolve(`const BUILD_ID = '${id}';`),
    })) as unknown as typeof fetch;
  }

  it('reloads cache-first (no SW round-trip, no cache bust) when already on the served build', async () => {
    stubServedBuildId('build-current'); // matches CLIENT_BUILD_ID
    refreshClient();
    await vi.advanceTimersByTimeAsync(0);

    expect(reload).toHaveBeenCalledTimes(1);
    // The whole point: don't probe the registration and don't drop the asset
    // cache when the loaded code is already current — that's the cold-load cost.
    expect(getRegistration).not.toHaveBeenCalled();
    expect(cacheDelete).not.toHaveBeenCalled();
  });

  it('falls back to the swap + bust dance when the served build is newer', async () => {
    stubServedBuildId('build-next'); // differs from CLIENT_BUILD_ID — stale
    refreshClient();
    await vi.advanceTimersByTimeAsync(0);

    expect(getRegistration).toHaveBeenCalledTimes(1);
    expect(cacheDelete).toHaveBeenCalledWith(CURRENT_SHELL_CACHE);
    expect(reload).toHaveBeenCalledTimes(1);
  });

  // A chunk that failed on the served build came out of the cache, so a
  // cache-first reload would load the same bad copy again.
  it('evicts the cached assets even on the served build when asked to', async () => {
    stubServedBuildId('build-current');
    refreshClient({ evictAssets: true });
    await vi.advanceTimersByTimeAsync(0);

    expect(cacheDelete).toHaveBeenCalledWith(CURRENT_SHELL_CACHE);
    expect(reload).toHaveBeenCalledTimes(1);
    expect(cacheDelete.mock.invocationCallOrder[0]).toBeLessThan(reload.mock.invocationCallOrder[0]);
  });

  // The UI stays locked until the reload, so a Cache API that never settles
  // must not hold it.
  it('still reloads when the eviction hangs', async () => {
    stubServedBuildId('build-current');
    Object.defineProperty(globalThis, 'caches', {
      value: { keys: () => new Promise(() => {}), delete: cacheDelete },
      configurable: true,
    });
    refreshClient({ evictAssets: true });
    await vi.advanceTimersByTimeAsync(1_999);
    expect(reload).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(reload).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(20_000);
    expect(reload).toHaveBeenCalledTimes(1);
  });

  // With no service worker there is no bundle cache, and nothing to swap.
  it('reloads plainly when no service worker is registered', async () => {
    globalThis.fetch = vi.fn(() => Promise.reject(new TypeError('Load failed'))) as unknown as typeof fetch;
    getRegistration.mockReturnValue(Promise.resolve(undefined));
    refreshClient({ evictAssets: true });
    await vi.advanceTimersByTimeAsync(0);

    expect(cacheDelete).not.toHaveBeenCalled();
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it('a stale-chunk reload evicts the cached assets', async () => {
    stubServedBuildId('build-current');
    sessionStorage.clear();
    expect(reloadForStaleChunk()).toBe(true);
    await vi.advanceTimersByTimeAsync(0);

    expect(cacheDelete).toHaveBeenCalledWith(CURRENT_SHELL_CACHE);
    expect(reload).toHaveBeenCalledTimes(1);
  });
});
