import {
  addPluginMarketplace,
  fetchPluginCatalog,
  isTransportError,
  removePluginMarketplace,
} from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { marketplaceCatalog, showToast } from '../store';
import { setLoadingIfFresh, toFailed } from '../types';
import type { MarketplaceCatalog } from '../types';

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

export function loadPluginCatalog(force = false): Promise<void> {
  if (!force && marketplaceCatalog.value.status === 'loaded') return Promise.resolve();
  if (catalogLoadInFlight) return catalogLoadInFlight;
  catalogLoadInFlight = (async () => {
    setLoadingIfFresh(marketplaceCatalog);
    try {
      marketplaceCatalog.value = { status: 'loaded', data: await fetchCatalogWithRetry() };
    } catch (e) {
      marketplaceCatalog.value = toFailed(e);
    }
  })().finally(() => { catalogLoadInFlight = null; });
  return catalogLoadInFlight;
}

export async function refreshPluginCatalog(): Promise<void> {
  await loadPluginCatalog(true);
}

export async function addPluginMarketplaceAction(source: string, name?: string): Promise<boolean> {
  const trimmed = source.trim();
  if (!trimmed) {
    showToast('Marketplace URL is required', 'error');
    return false;
  }
  try {
    await addPluginMarketplace(trimmed, name?.trim() || undefined);
    showToast('Marketplace registered', 'success');
    await refreshPluginCatalog();
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
    await removePluginMarketplace(id);
    showToast('Marketplace removed', 'success');
    await refreshPluginCatalog();
  } catch (e) {
    showToast(`Failed to remove marketplace: ${errorDetail(e)}`, 'error');
  }
}

// `installMarketplacePlugin` lives in `plugin-install.ts`, beside the opener it
// routes through, exactly as `uninstallMarketplacePlugin` lives in
// `plugin-uninstall.ts`. It cannot live here: this module is what both of those
// import `refreshPluginCatalog` from, so calling the opener from here would
// close an import cycle.
