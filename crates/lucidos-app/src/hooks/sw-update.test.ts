import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { shouldShowSwUpdateToast, markSwUpdateDismissed, scheduleServiceWorkerUpdateChecks, requestServiceWorkerBuildId } from './sw-update';

describe('SW update toast guard', () => {
  beforeEach(() => {
    sessionStorage.clear();
  });

  it('skips toast on initial install (no prior controller)', () => {
    expect(shouldShowSwUpdateToast(false)).toBe(false);
  });

  it('shows toast on genuine update (had controller at startup)', () => {
    expect(shouldShowSwUpdateToast(true)).toBe(true);
  });

  it('skips toast after dismiss flag is set', () => {
    markSwUpdateDismissed();
    expect(shouldShowSwUpdateToast(true)).toBe(false);
  });

  it('consumes dismiss flag (one-time guard)', () => {
    markSwUpdateDismissed();
    expect(shouldShowSwUpdateToast(true)).toBe(false); // consumed
    expect(shouldShowSwUpdateToast(true)).toBe(true);  // flag gone, next genuine update shows
  });

  it('dismiss flag has no effect on initial install', () => {
    markSwUpdateDismissed();
    expect(shouldShowSwUpdateToast(false)).toBe(false); // still blocked by hadController
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
