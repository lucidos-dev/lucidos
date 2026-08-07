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

import {
  loadPluginCatalog,
  refreshPluginCatalog,
  refreshPluginCatalogAfterMutation,
} from './plugin-marketplaces';

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

describe('catalog refresh coalescing', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    mockFetchPluginCatalog.mockReset();
    marketplaceCatalog.value = { status: 'not-loaded' };
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  /** A scan whose resolution the test controls, so a second call can be made
   *  while the first is genuinely in flight. */
  function deferredCatalog(marketplaces: Array<{ id: string; name: string; source: string }>) {
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    return {
      release,
      scan: async () => {
        await gate;
        return { marketplaces, plugins: [], errors: [] };
      },
    };
  }

  // The reported bug's exact shape. The agent registered a marketplace and
  // renamed it seconds later; a catalog scan git-clones every marketplace and
  // takes seconds, so the rename's SSE refresh lands mid-scan. Merely joining
  // the in-flight scan would settle the panel on the pre-rename registry that
  // scan had already read, with nothing left to correct it.
  it('re-scans after the in-flight one when a mutation refresh arrives mid-scan', async () => {
    const before = deferredCatalog([
      { id: 'm1', name: 'lucidos plugins', source: 'https://github.com/example-org/example-repo' },
    ]);
    mockFetchPluginCatalog
      .mockImplementationOnce(before.scan)
      .mockResolvedValueOnce({
        marketplaces: [
          { id: 'm1', name: "Example's plugins", source: 'https://github.com/example-org/example-repo' },
        ],
        plugins: [],
        errors: [],
      });

    // Registration event: scan starts and reads the pre-rename registry.
    const first = refreshPluginCatalogAfterMutation();
    // Rename event, while that scan is still cloning.
    const second = refreshPluginCatalogAfterMutation();

    before.release();
    await vi.runAllTimersAsync();
    await Promise.all([first, second]);
    await vi.runAllTimersAsync();

    expect(mockFetchPluginCatalog).toHaveBeenCalledTimes(2);
    expect(marketplaceCatalog.value).toMatchObject({
      status: 'loaded',
      data: { marketplaces: [{ name: "Example's plugins" }] },
    });
  });

  // Many events mid-scan collapse into ONE follow-up, not one scan per event.
  it('collapses a burst of mid-scan mutation refreshes into a single trailing scan', async () => {
    const gate = deferredCatalog([]);
    mockFetchPluginCatalog
      .mockImplementationOnce(gate.scan)
      .mockResolvedValue({ marketplaces: [], plugins: [], errors: [] });

    const first = refreshPluginCatalogAfterMutation();
    void refreshPluginCatalogAfterMutation();
    void refreshPluginCatalogAfterMutation();
    void refreshPluginCatalogAfterMutation();

    gate.release();
    await vi.runAllTimersAsync();
    await first;
    await vi.runAllTimersAsync();

    expect(mockFetchPluginCatalog).toHaveBeenCalledTimes(2);
  });

  // The case the in-flight sharing exists for. Neither a plain reader (the
  // AppsView prime-load) nor a panel-open re-scan knows of a mutation to be
  // fresher than, so neither may add a second clone-everything pass.
  it('does not queue a trailing scan for a reader or a plain re-scan', async () => {
    const gate = deferredCatalog([]);
    mockFetchPluginCatalog
      .mockImplementationOnce(gate.scan)
      .mockResolvedValue({ marketplaces: [], plugins: [], errors: [] });

    const first = refreshPluginCatalogAfterMutation();
    void loadPluginCatalog();
    void refreshPluginCatalog();

    gate.release();
    await vi.runAllTimersAsync();
    await first;
    await vi.runAllTimersAsync();

    expect(mockFetchPluginCatalog).toHaveBeenCalledTimes(1);
  });
});
