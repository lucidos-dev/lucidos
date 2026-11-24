import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { marketplaceCatalog, marketplaceScanning } from '../store';
import { ApiError } from '../../api/client/_core';
import type { MarketplaceCatalog } from '../types';
import { beginSliceRead, settleSliceRead } from '../bootstrapSliceLoads';

const mockFetchPluginCatalog = vi.fn();
const mockAddPluginMarketplace = vi.fn();
const mockRemovePluginMarketplace = vi.fn();
const mockRescanPluginCatalog = vi.fn();
vi.mock('../../api/client', () => ({
  rescanPluginCatalog: (...args: unknown[]) => mockRescanPluginCatalog(...args),
  fetchPluginCatalog: (...args: unknown[]) => mockFetchPluginCatalog(...args),
  addPluginMarketplace: (...args: unknown[]) => mockAddPluginMarketplace(...args),
  removePluginMarketplace: (...args: unknown[]) => mockRemovePluginMarketplace(...args),
  isTransportError: (err: unknown) =>
    err instanceof TypeError && /Load failed|Failed to fetch|NetworkError/i.test(err.message),
}));

import {
  addPluginMarketplaceAction,
  loadPluginCatalog,
  pluginCatalogScanned,
  pluginCatalogSlice,
  refreshPluginCatalog,
  refreshPluginCatalogAfterMutation,
  removePluginMarketplaceAction,
  rescanPluginCatalogAndSettle,
  resyncPluginCatalog,
} from './plugin-marketplaces';

function catalogOf(over: Partial<MarketplaceCatalog> = {}): MarketplaceCatalog {
  return {
    marketplaces: [],
    plugins: [],
    errors: [],
    scanned_at: '2026-09-22T10:00:00Z',
    scanning: false,
    scan_error: null,
    ...over,
  };
}

const emptyCatalog = catalogOf();

function deferredCatalog(
  marketplaces: Array<{ id: string; name: string; source: string }>,
  scanning = false,
) {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  return {
    release,
    scan: async () => {
      await gate;
      return catalogOf({ marketplaces, scanning });
    },
  };
}

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
    expect(mockFetchPluginCatalog).toHaveBeenCalledTimes(4);
  });
});

describe('a bootstrap snapshot of the catalog', () => {
  it('never overwrites a narrow catalog read that started after it', async () => {
    marketplaceCatalog.value = { status: 'not-loaded' };
    const start = beginSliceRead('plugin_catalog');
    const ctx = pluginCatalogSlice.begin();
    const newer = catalogOf({ scanned_at: '2026-09-22T11:00:00Z' });
    mockFetchPluginCatalog.mockReset().mockResolvedValueOnce(newer);
    await loadPluginCatalog(true);

    await settleSliceRead('plugin_catalog', start, pluginCatalogSlice, ctx, { body: emptyCatalog });

    expect(marketplaceCatalog.value).toEqual({ status: 'loaded', data: newer });
  });
});

describe('catalog refresh coalescing', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    mockFetchPluginCatalog.mockReset();
    marketplaceCatalog.value = { status: 'not-loaded' };
    marketplaceScanning.value = false;
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('takes the scanning flag from the engine, not from its own fetch', async () => {
    const first = deferredCatalog([], true);
    const trailing = deferredCatalog([], false);
    mockFetchPluginCatalog
      .mockImplementationOnce(first.scan)
      .mockImplementationOnce(trailing.scan);

    const firstRefresh = refreshPluginCatalogAfterMutation();
    void refreshPluginCatalogAfterMutation();

    first.release();
    await vi.runAllTimersAsync();
    await firstRefresh;

    expect(mockFetchPluginCatalog).toHaveBeenCalledTimes(2);
    expect(marketplaceScanning.value).toBe(true);

    trailing.release();
    await vi.runAllTimersAsync();

    expect(marketplaceScanning.value).toBe(false);
  });

  it('re-scans after the in-flight one when a mutation refresh arrives mid-scan', async () => {
    const before = deferredCatalog([
      { id: 'm1', name: 'lucidos plugins', source: 'https://github.com/example-org/example-repo' },
    ]);
    mockFetchPluginCatalog
      .mockImplementationOnce(before.scan)
      .mockResolvedValueOnce(catalogOf({
        marketplaces: [
          { id: 'm1', name: "Example's plugins", source: 'https://github.com/example-org/example-repo' },
        ],
      }));

    const first = refreshPluginCatalogAfterMutation();
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

  it('collapses a burst of mid-scan mutation refreshes into a single trailing scan', async () => {
    const gate = deferredCatalog([]);
    mockFetchPluginCatalog
      .mockImplementationOnce(gate.scan)
      .mockResolvedValue(emptyCatalog);

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

  it('does not queue a trailing scan for a reader or a plain re-scan', async () => {
    const gate = deferredCatalog([]);
    mockFetchPluginCatalog
      .mockImplementationOnce(gate.scan)
      .mockResolvedValue(emptyCatalog);

    const first = refreshPluginCatalogAfterMutation();
    void loadPluginCatalog();
    void refreshPluginCatalog();

    gate.release();
    await vi.runAllTimersAsync();
    await first;
    await vi.runAllTimersAsync();

    expect(mockFetchPluginCatalog).toHaveBeenCalledTimes(1);
  });
  it('settles a mid-scan mutation refresh only once its trailing read lands', async () => {
    const before = deferredCatalog([]);
    const trailing = deferredCatalog([{ id: 'm1', name: 'Fresh', source: 'https://github.com/example-org/example-repo' }]);
    mockFetchPluginCatalog
      .mockImplementationOnce(before.scan)
      .mockImplementationOnce(trailing.scan);

    void refreshPluginCatalogAfterMutation();
    let settled = false;
    const second = refreshPluginCatalogAfterMutation().then(() => { settled = true; });

    before.release();
    await vi.runAllTimersAsync();
    expect(settled).toBe(false);

    trailing.release();
    await second;
    expect(marketplaceCatalog.value).toMatchObject({
      status: 'loaded',
      data: { marketplaces: [{ name: 'Fresh' }] },
    });
  });
});

describe('a marketplace mutation shows its own result', () => {
  const NEW_MARKETPLACE = {
    id: 'example-repo-1a2b3c4d',
    name: 'Example plugins',
    source: 'https://github.com/example-org/example-repo',
  };
  const OTHER_MARKETPLACE = {
    id: 'other-repo-5e6f7a8b',
    name: 'Other plugins',
    source: 'https://github.com/example-org/other-repo',
  };

  function plugin(id: string, marketplace: { id: string; name: string; source: string }) {
    return {
      marketplace_id: marketplace.id,
      marketplace_name: marketplace.name,
      id,
      name: id,
      description: '',
      version: '1.0.0',
      source: marketplace.source,
      manifest: {},
      content: [],
      categories: [],
      files_count: 1,
      engine_compatible: true,
      status: 'available' as const,
      media: { screenshots: [], videos: [], problems: [] },
    };
  }

  function loadedWith(
    marketplaces: Array<{ id: string; name: string; source: string }>,
    plugins: ReturnType<typeof plugin>[] = [],
    errors: Array<{ marketplace_id: string; marketplace_name: string; source: string; error: string }> = [],
  ) {
    marketplaceCatalog.value = {
      status: 'loaded',
      data: catalogOf({ marketplaces, plugins, errors }),
    };
  }

  const openScans: Array<() => void> = [];
  function scanStillCloning() {
    const gate = deferredCatalog([]);
    openScans.push(gate.release);
    return gate.scan;
  }

  beforeEach(() => {
    vi.useFakeTimers();
    mockFetchPluginCatalog.mockReset();
    mockAddPluginMarketplace.mockReset();
    mockRemovePluginMarketplace.mockReset();
    marketplaceCatalog.value = { status: 'not-loaded' };
    marketplaceScanning.value = false;
  });
  afterEach(async () => {
    openScans.splice(0).forEach((release) => release());
    await vi.runAllTimersAsync();
    vi.useRealTimers();
  });

  it('registers and lists the marketplace without waiting for the scan', async () => {
    loadedWith([OTHER_MARKETPLACE]);
    mockAddPluginMarketplace.mockResolvedValue({
      marketplace: NEW_MARKETPLACE,
      marketplaces: [NEW_MARKETPLACE, OTHER_MARKETPLACE],
      created: true,
      commit: 'abc123',
    });
    mockFetchPluginCatalog.mockImplementation(scanStillCloning());

    let registered: boolean | undefined;
    void addPluginMarketplaceAction(NEW_MARKETPLACE.source).then((ok) => { registered = ok; });
    await vi.runAllTimersAsync();

    expect(registered).toBe(true);
    expect(marketplaceCatalog.value).toMatchObject({
      status: 'loaded',
      data: { marketplaces: [NEW_MARKETPLACE, OTHER_MARKETPLACE] },
    });
  });

  it('reports a scan running from the registration until its catalog lands', async () => {
    loadedWith([OTHER_MARKETPLACE]);
    mockAddPluginMarketplace.mockResolvedValue({
      marketplace: NEW_MARKETPLACE,
      marketplaces: [NEW_MARKETPLACE, OTHER_MARKETPLACE],
      created: true,
      commit: 'abc123',
    });
    const scanning = deferredCatalog([NEW_MARKETPLACE, OTHER_MARKETPLACE], true);
    const settled = deferredCatalog([NEW_MARKETPLACE, OTHER_MARKETPLACE], false);
    mockFetchPluginCatalog
      .mockImplementationOnce(scanning.scan)
      .mockImplementationOnce(settled.scan);

    await addPluginMarketplaceAction(NEW_MARKETPLACE.source);
    scanning.release();
    await vi.runAllTimersAsync();

    expect(marketplaceScanning.value).toBe(true);

    void refreshPluginCatalog();
    settled.release();
    await vi.runAllTimersAsync();

    expect(marketplaceScanning.value).toBe(false);
  });

  it('drops a removed marketplace and the rows attributed to it', async () => {
    loadedWith(
      [NEW_MARKETPLACE, OTHER_MARKETPLACE],
      [plugin('keep-me', OTHER_MARKETPLACE), plugin('drop-me', NEW_MARKETPLACE)],
      [{
        marketplace_id: NEW_MARKETPLACE.id,
        marketplace_name: NEW_MARKETPLACE.name,
        source: NEW_MARKETPLACE.source,
        error: 'no plugin manifest.toml files found',
      }],
    );
    mockRemovePluginMarketplace.mockResolvedValue({
      marketplaces: [OTHER_MARKETPLACE],
      removed: true,
      commit: 'abc123',
    });
    mockFetchPluginCatalog.mockImplementation(scanStillCloning());

    await removePluginMarketplaceAction(NEW_MARKETPLACE.id);
    await vi.runAllTimersAsync();

    expect(marketplaceCatalog.value).toMatchObject({
      status: 'loaded',
      data: { marketplaces: [OTHER_MARKETPLACE], plugins: [{ id: 'keep-me' }], errors: [] },
    });
  });

  it('discards a scan that predates the registration', async () => {
    loadedWith([OTHER_MARKETPLACE]);
    const stale = deferredCatalog([OTHER_MARKETPLACE]);
    mockFetchPluginCatalog
      .mockImplementationOnce(stale.scan)
      .mockResolvedValue(catalogOf({ marketplaces: [NEW_MARKETPLACE, OTHER_MARKETPLACE] }));
    void refreshPluginCatalog();

    mockAddPluginMarketplace.mockResolvedValue({
      marketplace: NEW_MARKETPLACE,
      marketplaces: [NEW_MARKETPLACE, OTHER_MARKETPLACE],
      created: true,
      commit: 'abc123',
    });
    await addPluginMarketplaceAction(NEW_MARKETPLACE.source);

    const seen: string[][] = [];
    const stopWatching = marketplaceCatalog.subscribe((catalog) => {
      if (catalog.status === 'loaded') seen.push(catalog.data.marketplaces.map((m) => m.id));
    });
    stale.release();
    await vi.runAllTimersAsync();
    stopWatching();

    expect(seen.length).toBeGreaterThan(0);
    expect(seen.every((ids) => ids.includes(NEW_MARKETPLACE.id))).toBe(true);
  });

  it('discards a scan carrying the pre-rename name', async () => {
    const renamed = { ...OTHER_MARKETPLACE, name: 'Renamed plugins' };
    loadedWith([OTHER_MARKETPLACE]);
    const stale = deferredCatalog([OTHER_MARKETPLACE]);
    mockFetchPluginCatalog
      .mockImplementationOnce(stale.scan)
      .mockResolvedValue(catalogOf({ marketplaces: [renamed] }));
    void refreshPluginCatalog();

    mockAddPluginMarketplace.mockResolvedValue({
      marketplace: renamed,
      marketplaces: [renamed],
      created: false,
      commit: 'abc123',
    });
    await addPluginMarketplaceAction(renamed.source, renamed.name);

    const seen: string[][] = [];
    const stopWatching = marketplaceCatalog.subscribe((catalog) => {
      if (catalog.status === 'loaded') seen.push(catalog.data.marketplaces.map((m) => m.name));
    });
    stale.release();
    await vi.runAllTimersAsync();
    stopWatching();

    expect(seen.length).toBeGreaterThan(0);
    expect(seen.every((names) => names.includes('Renamed plugins'))).toBe(true);
  });

  it('joins a scan that already reflects the registration', async () => {
    loadedWith([OTHER_MARKETPLACE]);
    const current = deferredCatalog([NEW_MARKETPLACE, OTHER_MARKETPLACE]);
    mockFetchPluginCatalog
      .mockImplementationOnce(current.scan)
      .mockResolvedValue(catalogOf({ marketplaces: [NEW_MARKETPLACE, OTHER_MARKETPLACE] }));
    void refreshPluginCatalogAfterMutation();

    mockAddPluginMarketplace.mockResolvedValue({
      marketplace: NEW_MARKETPLACE,
      marketplaces: [NEW_MARKETPLACE, OTHER_MARKETPLACE],
      created: true,
      commit: 'abc123',
    });
    await addPluginMarketplaceAction(NEW_MARKETPLACE.source);

    current.release();
    await vi.runAllTimersAsync();

    expect(mockFetchPluginCatalog).toHaveBeenCalledTimes(1);
    expect(marketplaceCatalog.value).toMatchObject({
      status: 'loaded',
      data: { marketplaces: [NEW_MARKETPLACE, OTHER_MARKETPLACE] },
    });
  });

  it('does not fabricate a catalog when none has loaded yet', async () => {
    mockAddPluginMarketplace.mockResolvedValue({
      marketplace: NEW_MARKETPLACE,
      marketplaces: [NEW_MARKETPLACE],
      created: true,
      commit: 'abc123',
    });
    mockFetchPluginCatalog.mockImplementation(scanStillCloning());

    await addPluginMarketplaceAction(NEW_MARKETPLACE.source);
    await vi.runAllTimersAsync();

    expect(marketplaceCatalog.value.status).toBe('loading');
  });
});

describe('a scan landing', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    mockFetchPluginCatalog.mockReset().mockResolvedValue(emptyCatalog);
    mockRescanPluginCatalog.mockReset();
    marketplaceScanning.value = true;
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('lowers the updating cue and re-reads an open catalog', async () => {
    marketplaceCatalog.value = { status: 'loaded', data: catalogOf({ scanned_at: '2026-09-22T09:00:00Z' }) };
    pluginCatalogScanned();
    expect(marketplaceScanning.value).toBe(false);
    await vi.runAllTimersAsync();
    expect(mockFetchPluginCatalog).toHaveBeenCalledTimes(1);
  });

  it('does not re-read a catalog nobody loaded', async () => {
    marketplaceCatalog.value = { status: 'not-loaded' };
    pluginCatalogScanned();
    await vi.runAllTimersAsync();
    expect(marketplaceScanning.value).toBe(false);
    expect(mockFetchPluginCatalog).not.toHaveBeenCalled();
  });

  it('refreshes by asking for a scan, and settles once its result is on screen', async () => {
    marketplaceScanning.value = false;
    marketplaceCatalog.value = { status: 'loaded', data: catalogOf({ scanned_at: '2026-09-22T09:00:00Z' }) };
    mockRescanPluginCatalog.mockResolvedValue({ queued: true });
    mockFetchPluginCatalog.mockResolvedValue(catalogOf({ scanned_at: '2026-09-22T10:05:00Z' }));

    let settled = false;
    const refresh = rescanPluginCatalogAndSettle().then(() => { settled = true; });
    await vi.runAllTimersAsync();
    expect(mockRescanPluginCatalog).toHaveBeenCalledTimes(1);
    expect(marketplaceScanning.value).toBe(true);
    expect(settled).toBe(false);

    pluginCatalogScanned();
    await refresh;
    expect(marketplaceCatalog.value).toMatchObject({ status: 'loaded', data: { scanned_at: '2026-09-22T10:05:00Z' } });
  });

  it('settles a refresh from a later read when the scan frame never arrives', async () => {
    marketplaceScanning.value = false;
    marketplaceCatalog.value = { status: 'loaded', data: catalogOf() };
    mockRescanPluginCatalog.mockResolvedValue({ queued: true });
    mockFetchPluginCatalog
      .mockResolvedValueOnce(catalogOf({ scanning: true }))
      .mockResolvedValueOnce(catalogOf({ scanned_at: '2026-09-22T10:05:00Z' }));

    let settled = false;
    const refresh = rescanPluginCatalogAndSettle().then(() => { settled = true; });
    await vi.runAllTimersAsync();

    await refreshPluginCatalog();
    await vi.runAllTimersAsync();
    expect(settled).toBe(false);

    await refreshPluginCatalog();
    await refresh;
    expect(marketplaceCatalog.value).toMatchObject({ status: 'loaded', data: { scanned_at: '2026-09-22T10:05:00Z' } });
  });

  it('does not settle on a scan that landed while the request was in flight', async () => {
    marketplaceScanning.value = false;
    marketplaceCatalog.value = { status: 'loaded', data: catalogOf() };
    let respond!: () => void;
    mockRescanPluginCatalog.mockReturnValue(new Promise((resolve) => { respond = () => resolve({ queued: true }); }));
    mockFetchPluginCatalog.mockResolvedValue(catalogOf({ scanning: true }));

    let settled = false;
    const refresh = rescanPluginCatalogAndSettle().then(() => { settled = true; });
    pluginCatalogScanned();
    respond();
    await vi.runAllTimersAsync();
    expect(settled).toBe(false);
    expect(marketplaceScanning.value).toBe(true);

    mockFetchPluginCatalog.mockResolvedValue(catalogOf({ scanned_at: '2026-09-22T10:05:00Z' }));
    pluginCatalogScanned();
    await refresh;
    expect(marketplaceCatalog.value).toMatchObject({ status: 'loaded', data: { scanned_at: '2026-09-22T10:05:00Z' } });
  });

  it('does not settle on a frame whose re-read still finds a scan running', async () => {
    marketplaceScanning.value = false;
    marketplaceCatalog.value = { status: 'loaded', data: catalogOf() };
    mockRescanPluginCatalog.mockResolvedValue({ queued: true });
    mockFetchPluginCatalog.mockResolvedValue(catalogOf({ scanning: true }));

    let settled = false;
    const refresh = rescanPluginCatalogAndSettle().then(() => { settled = true; });
    await vi.runAllTimersAsync();
    pluginCatalogScanned();
    await vi.runAllTimersAsync();
    expect(settled).toBe(false);

    mockFetchPluginCatalog.mockResolvedValue(catalogOf({ scanned_at: '2026-09-22T10:05:00Z' }));
    pluginCatalogScanned();
    await refresh;
    expect(marketplaceCatalog.value).toMatchObject({ status: 'loaded', data: { scanned_at: '2026-09-22T10:05:00Z' } });
  });

  it('settles on a read after the request that fails', async () => {
    marketplaceScanning.value = false;
    marketplaceCatalog.value = { status: 'loaded', data: catalogOf() };
    mockRescanPluginCatalog.mockResolvedValue({ queued: true });
    mockFetchPluginCatalog.mockRejectedValue(new ApiError(500, 'read cache: boom'));

    const refresh = rescanPluginCatalogAndSettle();
    await vi.runAllTimersAsync();
    resyncPluginCatalog();
    await vi.runAllTimersAsync();
    await refresh;
    expect(marketplaceCatalog.value.status).toBe('failed');
  });

  it('re-reads an open catalog on reconnect, and leaves an unopened one alone', async () => {
    marketplaceCatalog.value = { status: 'not-loaded' };
    resyncPluginCatalog();
    await vi.runAllTimersAsync();
    expect(mockFetchPluginCatalog).not.toHaveBeenCalled();

    marketplaceCatalog.value = { status: 'loaded', data: catalogOf() };
    resyncPluginCatalog();
    await vi.runAllTimersAsync();
    expect(mockFetchPluginCatalog).toHaveBeenCalledTimes(1);
  });

  it('fails a refresh whose scan request fails, and lowers the cue', async () => {
    marketplaceScanning.value = false;
    marketplaceCatalog.value = { status: 'loaded', data: catalogOf() };
    mockRescanPluginCatalog.mockRejectedValue(new ApiError(500, 'scan queue: boom'));

    await expect(rescanPluginCatalogAndSettle()).rejects.toThrow('boom');
    expect(marketplaceScanning.value).toBe(false);
  });
});
