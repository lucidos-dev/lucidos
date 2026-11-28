import {
  addPluginMarketplace,
  fetchPluginCatalog,
  isTransportError,
  removePluginMarketplace,
  rescanPluginCatalog,
} from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { marketplaceCatalog, marketplaceScanning, showToast } from '../store';
import { setLoadingIfFresh, toFailed } from '../types';
import { readSlice, startSliceLoad, type SliceOf } from '../bootstrapSliceLoads';
import type { MarketplaceCatalog, PluginMarketplace } from '../types';

export const OFFICIAL_MARKETPLACE = {
  source: 'https://github.com/lucidos-dev/plugins',
  name: 'Lucidos plugins',
} as const;

let catalogLoadInFlight: Promise<void> | null = null;

let catalogReadSeq = 0;

const CATALOG_RETRY_BACKOFFS_MS = [800, 1600, 3200];

function isTransientCatalogError(e: unknown): boolean {
  return isTransportError(e) || (e instanceof DOMException && e.name === 'TimeoutError');
}

async function fetchCatalogWithRetry(): Promise<MarketplaceCatalog> {
  for (let attempt = 0; ; attempt++) {
    try {
      return await fetchPluginCatalog();
    } catch (e) {
      if (attempt >= CATALOG_RETRY_BACKOFFS_MS.length || !isTransientCatalogError(e)) throw e;
      await new Promise((resolve) => setTimeout(resolve, CATALOG_RETRY_BACKOFFS_MS[attempt]));
    }
  }
}

let catalogRefreshQueued = false;

let expectedMarketplaces: string | null = null;

function marketplaceKey(marketplaces: PluginMarketplace[]): string {
  return marketplaces
    .map((m) => [m.id, m.name, m.source].join(' '))
    .sort()
    .join('\n');
}

function scanRanBehindAMutation(scanned: PluginMarketplace[]): boolean {
  const expected = expectedMarketplaces;
  expectedMarketplaces = null;
  return expected !== null && marketplaceKey(scanned) !== expected;
}

export function loadPluginCatalog(force = false): Promise<void> {
  if (!force && marketplaceCatalog.value.status === 'loaded') return Promise.resolve();
  if (catalogLoadInFlight) return catalogLoadInFlight;
  const joined = startSliceLoad('plugin_catalog');
  if (joined) return joined;
  catalogLoadInFlight = readSlice('plugin_catalog', pluginCatalogSlice, () => fetchCatalogWithRetry()).finally(() => {
    catalogLoadInFlight = null;
    startQueuedCatalogRefresh();
  });
  return catalogLoadInFlight;
}

function beginCatalogRead(): number {
  const readSeq = ++catalogReadSeq;
  setLoadingIfFresh(marketplaceCatalog);
  return readSeq;
}

function applyCatalog(catalog: MarketplaceCatalog, readSeq: number): void {
  marketplaceScanning.value = catalog.scanning;
  if (scanRanBehindAMutation(catalog.marketplaces)) {
    catalogRefreshQueued = true;
    return;
  }
  marketplaceCatalog.value = { status: 'loaded', data: catalog };
  if (!catalog.scanning) settleScanWaitersReadAfter(readSeq);
}

function failCatalog(e: unknown, readSeq: number): void {
  expectedMarketplaces = null;
  marketplaceCatalog.value = toFailed(e);
  settleScanWaitersReadAfter(readSeq);
}

function startQueuedCatalogRefresh(): void {
  if (catalogLoadInFlight || !catalogRefreshQueued) return;
  catalogRefreshQueued = false;
  void loadPluginCatalog(true);
}

export const pluginCatalogSlice: SliceOf<'plugin_catalog', number> = {
  begin: beginCatalogRead,
  apply: (catalog, readSeq) => {
    applyCatalog(catalog, readSeq);
    startQueuedCatalogRefresh();
  },
  fail: (e, readSeq) => {
    failCatalog(e, readSeq);
    startQueuedCatalogRefresh();
  },
};

export async function refreshPluginCatalog(): Promise<void> {
  await loadPluginCatalog(true);
}

export async function rescanPluginCatalogAction(): Promise<void> {
  try {
    await rescanPluginCatalogAndSettle();
  } catch (e) {
    showToast(`Failed to start a marketplace scan: ${errorDetail(e)}`, 'error');
  }
}

export async function refreshPluginCatalogAfterMutation(): Promise<void> {
  if (catalogLoadInFlight) {
    catalogRefreshQueued = true;
    await catalogLoadInFlight;
    await catalogLoadInFlight;
    return;
  }
  await loadPluginCatalog(true);
}

interface ScanWaiter {
  settle: () => void;
  afterRead: number | null;
  landedEarly: boolean;
}

let scanWaiters: ScanWaiter[] = [];

function settleScanWaitersReadAfter(readSeq: number): void {
  const done = scanWaiters.filter((w) => w.afterRead !== null && readSeq > w.afterRead);
  if (done.length === 0) return;
  scanWaiters = scanWaiters.filter((w) => !done.includes(w));
  for (const w of done) w.settle();
}

export function pluginCatalogScanned(): void {
  marketplaceScanning.value = false;
  for (const w of scanWaiters) if (w.afterRead === null) w.landedEarly = true;
  resyncPluginCatalog();
}

export function resyncPluginCatalog(): void {
  if (marketplaceCatalog.value.status === 'loaded' || scanWaiters.length > 0) {
    void refreshPluginCatalogAfterMutation();
  }
}

export async function rescanPluginCatalogAndSettle(): Promise<void> {
  let settle!: () => void;
  const landed = new Promise<void>((resolve) => { settle = resolve; });
  const waiter: ScanWaiter = { settle, afterRead: null, landedEarly: false };
  scanWaiters.push(waiter);
  marketplaceScanning.value = true;
  try {
    await rescanPluginCatalog();
  } catch (e) {
    scanWaiters = scanWaiters.filter((w) => w !== waiter);
    marketplaceScanning.value = false;
    throw e;
  }
  waiter.afterRead = catalogReadSeq;
  if (waiter.landedEarly || catalogLoadInFlight) void refreshPluginCatalogAfterMutation();
  await landed;
}

function applyMarketplaceMutation(marketplaces: PluginMarketplace[]): void {
  expectedMarketplaces = marketplaceKey(marketplaces);
  const current = marketplaceCatalog.value;
  if (current.status === 'loaded') {
    const registered = new Set(marketplaces.map((m) => m.id));
    marketplaceCatalog.value = {
      status: 'loaded',
      data: {
        ...current.data,
        marketplaces,
        plugins: current.data.plugins.filter((p) => registered.has(p.marketplace_id)),
        errors: current.data.errors.filter((e) => registered.has(e.marketplace_id)),
      },
    };
  }
  void refreshPluginCatalog();
}

async function postMarketplace(
  source: string,
  name: string | undefined,
  verb: 'register' | 'rename',
): Promise<boolean> {
  try {
    const { marketplaces } = await addPluginMarketplace(source, name);
    applyMarketplaceMutation(marketplaces);
    showToast(verb === 'rename' ? 'Marketplace renamed' : 'Marketplace registered', 'success');
    return true;
  } catch (e) {
    showToast(`Failed to ${verb} marketplace: ${errorDetail(e)}`, 'error');
    return false;
  }
}

export async function addPluginMarketplaceAction(source: string, name?: string): Promise<boolean> {
  const trimmed = source.trim();
  if (!trimmed) {
    showToast('Marketplace URL is required', 'error');
    return false;
  }
  return postMarketplace(trimmed, name?.trim() || undefined, 'register');
}

export function renamePluginMarketplaceAction(source: string, name: string): Promise<boolean> {
  return postMarketplace(source, name, 'rename');
}

export function addOfficialMarketplaceAction(): Promise<boolean> {
  return addPluginMarketplaceAction(OFFICIAL_MARKETPLACE.source, OFFICIAL_MARKETPLACE.name);
}

export async function removePluginMarketplaceAction(id: string): Promise<void> {
  try {
    const { marketplaces } = await removePluginMarketplace(id);
    applyMarketplaceMutation(marketplaces);
    showToast('Marketplace removed', 'success');
  } catch (e) {
    showToast(`Failed to remove marketplace: ${errorDetail(e)}`, 'error');
  }
}
