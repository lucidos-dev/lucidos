import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { marketplaceCatalog } from '../store';
import { ApiError } from '../../api/client/_core';

// Mock the API client barrel. `fetchPluginCatalog` is the slow catalog scan
// (clones every registered marketplace repo) that the bug surfaces on; the rest
// are pulled in by plugin-marketplaces.ts at import time. `isTransportError`
// keeps the REAL transport-error classifier so the retry path is exercised
// honestly rather than against a stub that always says "transient".
const mockFetchPluginCatalog = vi.fn();
vi.mock('../../api/client', () => ({
  fetchPluginCatalog: (...args: unknown[]) => mockFetchPluginCatalog(...args),
  addPluginMarketplace: vi.fn(),
  removePluginMarketplace: vi.fn(),
  isTransportError: (err: unknown) =>
    err instanceof TypeError && /Load failed|Failed to fetch|NetworkError/i.test(err.message),
}));

import { loadPluginCatalog } from './plugin-marketplaces';

const emptyCatalog = { marketplaces: [], plugins: [], errors: [] };

describe('loadPluginCatalog self-heal', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    mockFetchPluginCatalog.mockReset();
    marketplaceCatalog.value = { status: 'not-loaded' };
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('retries a transient transport error (iOS PWA "Load failed") and self-heals to loaded', async () => {
    // Two stale-connection blips, then the connection re-establishes — exactly
    // what the user sees when they navigate away and back and the panel loads.
    mockFetchPluginCatalog
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce(emptyCatalog);

    const done = loadPluginCatalog(true);
    await vi.runAllTimersAsync();
    await done;

    expect(marketplaceCatalog.value.status).toBe('loaded');
    expect(mockFetchPluginCatalog).toHaveBeenCalledTimes(3);
  });

  it('does NOT retry a genuine server error — it surfaces as failed', async () => {
    mockFetchPluginCatalog.mockRejectedValue(new ApiError(500, 'scan marketplaces: boom'));

    const done = loadPluginCatalog(true);
    await vi.runAllTimersAsync();
    await done;

    expect(marketplaceCatalog.value.status).toBe('failed');
    expect(mockFetchPluginCatalog).toHaveBeenCalledTimes(1);
  });

  it('gives up after exhausting retries on a persistent transport error', async () => {
    mockFetchPluginCatalog.mockRejectedValue(new TypeError('Load failed'));

    const done = loadPluginCatalog(true);
    await vi.runAllTimersAsync();
    await done;

    expect(marketplaceCatalog.value.status).toBe('failed');
    // 1 initial attempt + the bounded retries.
    expect(mockFetchPluginCatalog).toHaveBeenCalledTimes(4);
  });
});
