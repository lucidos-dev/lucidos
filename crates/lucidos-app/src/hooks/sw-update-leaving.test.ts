import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// The leaving time is module state, so it gets a file of its own and cannot
// leak into the other stale-chunk tests.
import { NAVIGATION_CANCEL_WINDOW_MS, reloadForStaleChunk } from './sw-update';

describe('a chunk that fails as the page leaves', () => {
  const originalLocation = (globalThis as { location?: unknown }).location;
  let reload: ReturnType<typeof vi.fn>;
  beforeEach(() => {
    vi.useFakeTimers();
    reload = vi.fn();
    Object.defineProperty(globalThis, 'location', { value: { reload }, configurable: true });
    sessionStorage.clear();
  });
  afterEach(() => {
    vi.useRealTimers();
    Object.defineProperty(globalThis, 'location', { value: originalLocation, configurable: true });
  });

  // WebKit cancels the page's loads as a navigation starts, right after
  // `beforeunload`. A reload then would cancel the navigation instead.
  it('does not reload, and leaves the loop guard for a real stale chunk', () => {
    window.dispatchEvent(new Event('beforeunload'));

    expect(reloadForStaleChunk()).toBe(true);
    expect(reload).not.toHaveBeenCalled();
    expect(sessionStorage.length).toBe(0);
  });

  // A download also fires `beforeunload`, and the page stays.
  it('reloads for a stale chunk once the page has stayed past the window', () => {
    window.dispatchEvent(new Event('beforeunload'));
    vi.advanceTimersByTime(NAVIGATION_CANCEL_WINDOW_MS);

    expect(reloadForStaleChunk()).toBe(true);
    expect(reload).toHaveBeenCalledTimes(1);
  });
});
