import {
  addPluginMarketplace,
  fetchPluginCatalog,
  isTransportError,
  removePluginMarketplace,
} from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { marketplaceCatalog, marketplaceScanning, showToast } from '../store';
import { setLoadingIfFresh, toFailed } from '../types';
import type { MarketplaceCatalog, PluginMarketplace } from '../types';

/** The official Lucidos plugin marketplace. Suggested as a one-click add in the
 *  Plugins panel catalog and Settings → Marketplaces empty states so a fresh workspace has a
 *  marketplace to install plugins from without hunting for a URL. */
export const OFFICIAL_MARKETPLACE = {
  source: 'https://github.com/lucidos-dev/plugins',
  name: 'Lucidos plugins',
} as const;

// Share an in-flight scan so concurrent callers await the SAME fetch. Without
// this, opening Apps on the Store tab fires two catalog scans at once — the
// AppsView prime-load (for installed-app marketplace labels) and the StoreTab
// refresh — each cloning every registered marketplace repo.
let catalogLoadInFlight: Promise<void> | null = null;

// The catalog scan clones every registered marketplace repo, so on a flaky link
// (an iOS PWA resuming over Tailscale) the GET can fail at the transport layer
// — Safari surfaces this as `TypeError: "Load failed"` — or time out client-side
// before the engine answers. Both recover on their own moments later: the user
// sees exactly this when they navigate away and back and the panel then loads
// fine. So retry these transient failures with a short backoff before settling
// the Loadable to `failed`, keeping it in `loading` (skeleton) meanwhile, rather
// than leaving a terminal error that only a manual remount clears. A genuine
// server error (the engine's `{error}` body, an `ApiError`) is NOT transient and
// surfaces immediately. The service worker already retries GETs once, but only
// immediately — too soon for a connection that needs a beat to re-establish.
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

// Set when a caller that KNOWS something just changed arrives mid-scan (see
// `refreshPluginCatalogAfterMutation`). Drained by whichever scan is in flight
// when it settles. A single flag rather than a queue, so a burst of events
// collapses into ONE follow-up scan; it is cleared before that follow-up
// starts, so a steady stream can never build a backlog or spin.
let catalogRefreshQueued = false;

// The marketplace list the latest local mutation put on screen, set by
// `applyMarketplaceMutation` and consumed by the next scan to finish. A scan
// disagreeing with it read the registry before that write, so its list is older
// than one already on screen: drop it, or the row the user just added blinks
// out. Consumed whatever it says, so a peer's concurrent change costs one extra
// scan instead of looping.
let expectedMarketplaces: string | null = null;

/** Every field a mutation can change, not just the id. A marketplace id is a
 *  hash of its canonical source, so re-registering a source under a new name
 *  keeps the id. An id-only key is therefore blind to a rename, which is the
 *  mutation that had the panel showing a stale name in the first place. Both
 *  lists come from the one registry file, so equal values compare identical. */
function marketplaceKey(marketplaces: PluginMarketplace[]): string {
  return marketplaces
    .map((m) => [m.id, m.name, m.source].join(' '))
    .sort()
    .join('\n');
}

/** Did this scan come back with an older marketplace list than the one a local
 *  mutation already applied? Consumes the expectation either way.
 *
 *  The engine writes the registry before it announces, so the scan our own SSE
 *  frame started reads the new registry and passes. Only a scan that began
 *  before the write is dropped. */
function scanRanBehindAMutation(scanned: PluginMarketplace[]): boolean {
  const expected = expectedMarketplaces;
  expectedMarketplaces = null;
  return expected !== null && marketplaceKey(scanned) !== expected;
}

/** Load the catalog. `force` re-reads even when it is already loaded, but a
 *  call landing during another scan still JOINS that scan: this is the reader's
 *  entry point, and a reader has no mutation to be fresher than. Use
 *  `refreshPluginCatalogAfterMutation` when the caller does. */
export function loadPluginCatalog(force = false): Promise<void> {
  if (!force && marketplaceCatalog.value.status === 'loaded') return Promise.resolve();
  if (catalogLoadInFlight) return catalogLoadInFlight;
  catalogLoadInFlight = (async () => {
    setLoadingIfFresh(marketplaceCatalog);
    marketplaceScanning.value = true;
    try {
      const catalog = await fetchCatalogWithRetry();
      if (scanRanBehindAMutation(catalog.marketplaces)) {
        // Queue the re-scan here rather than trust the caller to have queued
        // one, so a dropped result can never leave the panel with none coming.
        catalogRefreshQueued = true;
        return;
      }
      marketplaceCatalog.value = { status: 'loaded', data: catalog };
    } catch (e) {
      // The applied list goes with the catalog it lived in, so there is nothing
      // left for a later scan to be checked against.
      expectedMarketplaces = null;
      marketplaceCatalog.value = toFailed(e);
    }
  })().finally(() => {
    catalogLoadInFlight = null;
    if (catalogRefreshQueued) {
      catalogRefreshQueued = false;
      void loadPluginCatalog(true);
      return;
    }
    // Cleared only when nothing follows, so a trailing scan reads as one
    // continuous scan rather than blinking the flag off between the two.
    marketplaceScanning.value = false;
  });
  return catalogLoadInFlight;
}

/** Re-scan now, for a caller that just wants current data (the Plugins panel
 *  opening, its 5-minute poll). Joining an in-flight scan is good enough: there
 *  is no specific change it has to be newer than. */
export async function refreshPluginCatalog(): Promise<void> {
  await loadPluginCatalog(true);
}

/** Re-scan for a caller reacting to a mutation that has ALREADY landed: a
 *  marketplace registered/renamed/removed (locally or over SSE), a plugin
 *  installed or uninstalled.
 *
 *  Such a caller must not settle on a scan that started before its mutation.
 *  That scan already read the registry, so joining it silently lands pre-change
 *  data with nothing left to correct it, and the reported bug is exactly that
 *  shape: an agent registered a marketplace and renamed it seconds later, well
 *  inside one scan (each scan git-clones every registered marketplace), so the
 *  rename's refresh joined the registration's scan and the panel kept the OLD
 *  name. So a mid-scan arrival queues a trailing re-scan instead of joining.
 *
 *  Deliberately NOT what `refreshPluginCatalog` does. The trailing scan is a
 *  second clone-everything pass, and spending it on a caller with nothing to be
 *  fresher than is what the in-flight sharing above exists to prevent. */
export async function refreshPluginCatalogAfterMutation(): Promise<void> {
  if (catalogLoadInFlight) {
    catalogRefreshQueued = true;
    await catalogLoadInFlight;
    return;
  }
  await loadPluginCatalog(true);
}

/** Show the marketplace list an add/remove response carried, then fill in the
 *  plugins behind it.
 *
 *  A scan git-clones every registered marketplace, so it takes seconds per
 *  repo, and the write it will report is already committed when the response
 *  returns. Waiting for one before showing the row is what left the Add form
 *  disabled with the URL still in it. Reasoning and rejected variants:
 *  `docs/plans/2026-09-15-marketplace-add-must-not-wait-on-the-catalog-scan.md`.
 *
 *  Plugins and scan errors still come from a scan. Only those under a
 *  marketplace that is no longer registered are dropped here, since keeping
 *  them would offer a removed marketplace's plugins as installable. A catalog
 *  that is not yet `loaded` shows nothing: synthesising one with no plugins
 *  would swap the Store tab's skeleton for an empty state the scan contradicts.
 *
 *  The list is recorded as the expectation either way. THAT is what lets the
 *  rescan join an in-flight scan rather than queue a trailing one. So the two
 *  are one function, not a pairing a caller could split. */
function applyMarketplaceMutation(marketplaces: PluginMarketplace[]): void {
  expectedMarketplaces = marketplaceKey(marketplaces);
  const current = marketplaceCatalog.value;
  if (current.status === 'loaded') {
    const registered = new Set(marketplaces.map((m) => m.id));
    marketplaceCatalog.value = {
      status: 'loaded',
      data: {
        marketplaces,
        plugins: current.data.plugins.filter((p) => registered.has(p.marketplace_id)),
        errors: current.data.errors.filter((e) => registered.has(e.marketplace_id)),
      },
    };
  }
  // Never awaited: the mutation itself is done, so holding a form open for a
  // clone-everything pass is the freeze this fixes.
  void refreshPluginCatalog();
}

export async function addPluginMarketplaceAction(source: string, name?: string): Promise<boolean> {
  const trimmed = source.trim();
  if (!trimmed) {
    showToast('Marketplace URL is required', 'error');
    return false;
  }
  try {
    const { marketplaces } = await addPluginMarketplace(trimmed, name?.trim() || undefined);
    applyMarketplaceMutation(marketplaces);
    showToast('Marketplace registered', 'success');
    return true;
  } catch (e) {
    showToast(`Failed to register marketplace: ${errorDetail(e)}`, 'error');
    return false;
  }
}

/** One-click register the official Lucidos marketplace (the empty-state
 *  suggestion). Reuses addPluginMarketplaceAction so it gets the same toast +
 *  catalog refresh; the backend is idempotent on the URL, so a double-click
 *  re-registers the same entry harmlessly. */
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

// `installMarketplacePlugin` lives in `plugin-install.ts`, beside the opener it
// routes through, exactly as `uninstallMarketplacePlugin` lives in
// `plugin-uninstall.ts`. It cannot live here: this module is what both of those
// import their catalog refresh from, so calling the opener from here would
// close an import cycle.
