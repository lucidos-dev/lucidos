import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// Pin the running bundle's build id so the fast path can match (or miss) the
// served sw.js BUILD_ID deterministically — in a real build the
// `lucidos-sw-stamp` plugin replaces the `__LUCIDOS_BUILD_ID__` placeholder with
// this exact value. Must be hoisted above the import of the module under test.
vi.mock('virtual:build-id', () => ({ CLIENT_BUILD_ID: 'build-current' }));

import { refreshClient, isRunningServedBuild } from './sw-update';

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
      value: { keys: () => Promise.resolve(['lucidos-shell-build-current']), delete: cacheDelete },
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
    expect(cacheDelete).toHaveBeenCalledWith('lucidos-shell-build-current');
    expect(reload).toHaveBeenCalledTimes(1);
  });
});
