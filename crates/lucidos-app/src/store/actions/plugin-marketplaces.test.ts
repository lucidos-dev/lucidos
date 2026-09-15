import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { marketplaceCatalog, marketplaceScanning } from '../store';
import { ApiError } from '../../api/client/_core';

// Mock the API client barrel. `fetchPluginCatalog` is the slow catalog scan
// (clones every registered marketplace repo) that the bug surfaces on; the rest
// are pulled in by plugin-marketplaces.ts at import time. `isTransportError`
// keeps the REAL transport-error classifier so the retry path is exercised
// honestly rather than against a stub that always says "transient".
const mockFetchPluginCatalog = vi.fn();
const mockAddPluginMarketplace = vi.fn();
const mockRemovePluginMarketplace = vi.fn();
vi.mock('../../api/client', () => ({
  fetchPluginCatalog: (...args: unknown[]) => mockFetchPluginCatalog(...args),
  addPluginMarketplace: (...args: unknown[]) => mockAddPluginMarketplace(...args),
  removePluginMarketplace: (...args: unknown[]) => mockRemovePluginMarketplace(...args),
  isTransportError: (err: unknown) =>
    err instanceof TypeError && /Load failed|Failed to fetch|NetworkError/i.test(err.message),
}));

import {
  addPluginMarketplaceAction,
  loadPluginCatalog,
  refreshPluginCatalog,
  refreshPluginCatalogAfterMutation,
  removePluginMarketplaceAction,
} from './plugin-marketplaces';

const emptyCatalog = { marketplaces: [], plugins: [], errors: [] };

/** A scan whose resolution the test controls, so work can be done while one is
 *  genuinely in flight. Left unreleased, it stands in for the seconds a real
 *  scan spends cloning every registered marketplace. */
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
    marketplaceScanning.value = false;
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  // The Store tab tells "no plugins" from "not scanned yet" off this flag. A dip
  // between a scan and its trailing re-scan would flash "No plugins found" in
  // the middle of one continuous scan.
  it('holds the scanning flag up across the trailing re-scan', async () => {
    const first = deferredCatalog([]);
    const trailing = deferredCatalog([]);
    mockFetchPluginCatalog
      .mockImplementationOnce(first.scan)
      .mockImplementationOnce(trailing.scan);

    const firstRefresh = refreshPluginCatalogAfterMutation();
    void refreshPluginCatalogAfterMutation();

    first.release();
    await vi.runAllTimersAsync();
    await firstRefresh;

    // The trailing scan is the one cloning now, so the flag must still be up.
    expect(mockFetchPluginCatalog).toHaveBeenCalledTimes(2);
    expect(marketplaceScanning.value).toBe(true);

    trailing.release();
    await vi.runAllTimersAsync();

    expect(marketplaceScanning.value).toBe(false);
  });

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

// The reported freeze: the Add form stayed disabled with the URL still in it,
// and the new row absent, for as long as a clone-everything scan took. The
// registry write is already committed when the response returns, and the
// response carries the whole list, so neither has to wait for a scan.
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
      status: 'available' as const,
    };
  }

  function loadedWith(
    marketplaces: Array<{ id: string; name: string; source: string }>,
    plugins: ReturnType<typeof plugin>[] = [],
    errors: Array<{ marketplace_id: string; marketplace_name: string; source: string; error: string }> = [],
  ) {
    marketplaceCatalog.value = { status: 'loaded', data: { marketplaces, plugins, errors } };
  }

  // A scan left unresolved would outlive its test: the in-flight handle is
  // module state, so the next test's refresh would join it instead of starting
  // one. Every gate opened here is released when the test ends.
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
    // Never released: the scan is still cloning when the assertions run.
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

  // The registered marketplace is listed with no plugins under it yet. So the
  // Store tab must know a scan is running before it says "No plugins found".
  it('reports a scan running from the registration until its catalog lands', async () => {
    loadedWith([OTHER_MARKETPLACE]);
    mockAddPluginMarketplace.mockResolvedValue({
      marketplace: NEW_MARKETPLACE,
      marketplaces: [NEW_MARKETPLACE, OTHER_MARKETPLACE],
      created: true,
      commit: 'abc123',
    });
    const scan = deferredCatalog([NEW_MARKETPLACE, OTHER_MARKETPLACE]);
    mockFetchPluginCatalog.mockImplementation(scan.scan);

    await addPluginMarketplaceAction(NEW_MARKETPLACE.source);
    expect(marketplaceScanning.value).toBe(true);

    scan.release();
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

  // A scan that read the registry BEFORE the write must not land: its list has
  // no new marketplace in it, so the row the user just added would blink out
  // until the trailing scan restored it.
  it('discards a scan that predates the registration', async () => {
    loadedWith([OTHER_MARKETPLACE]);
    const stale = deferredCatalog([OTHER_MARKETPLACE]);
    mockFetchPluginCatalog
      .mockImplementationOnce(stale.scan)
      .mockResolvedValue({
        marketplaces: [NEW_MARKETPLACE, OTHER_MARKETPLACE],
        plugins: [],
        errors: [],
      });
    // The Plugins panel's own re-scan, already cloning when the user hits Add.
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

  // Re-registering a source under a new name is a rename, and the id is a hash
  // of the source, so the id survives it. An expectation keyed on ids alone
  // would accept a scan carrying the OLD name, which is the stale-name bug
  // `refreshPluginCatalogAfterMutation` exists for.
  it('discards a scan carrying the pre-rename name', async () => {
    const renamed = { ...OTHER_MARKETPLACE, name: 'Renamed plugins' };
    loadedWith([OTHER_MARKETPLACE]);
    const stale = deferredCatalog([OTHER_MARKETPLACE]);
    mockFetchPluginCatalog
      .mockImplementationOnce(stale.scan)
      .mockResolvedValue({ marketplaces: [renamed], plugins: [], errors: [] });
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

  // The engine writes the registry before it announces, so the scan our own
  // SSE frame starts already reads the new one. Queueing a trailing scan
  // beside it would clone every registered repo a second time for nothing.
  it('joins a scan that already reflects the registration', async () => {
    loadedWith([OTHER_MARKETPLACE]);
    const current = deferredCatalog([NEW_MARKETPLACE, OTHER_MARKETPLACE]);
    mockFetchPluginCatalog
      .mockImplementationOnce(current.scan)
      .mockResolvedValue({
        marketplaces: [NEW_MARKETPLACE, OTHER_MARKETPLACE],
        plugins: [],
        errors: [],
      });
    // The SSE frame this device's own registration produced, answered before
    // the POST resolved.
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

  // Synthesising a loaded catalog with no plugins would swap the Store tab's
  // skeleton for an empty state the scan then contradicts.
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
