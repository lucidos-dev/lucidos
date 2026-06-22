import {
  addPluginMarketplace,
  fetchPluginCatalog,
  removePluginMarketplace,
  stagePluginInstall,
} from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { marketplaceCatalog, panelOverlay, showToast } from '../store';
import { setLoadingIfFresh, toFailed } from '../types';
import type { MarketplacePlugin } from '../types';
import { pushNavState } from './navigation';
import { revealContentPane } from './pane';

/** The official Lucidos plugin marketplace. Suggested as a one-click add in the
 *  App Store and Settings → Marketplaces empty states so a fresh workspace has a
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

export function loadPluginCatalog(force = false): Promise<void> {
  if (!force && marketplaceCatalog.value.status === 'loaded') return Promise.resolve();
  if (catalogLoadInFlight) return catalogLoadInFlight;
  catalogLoadInFlight = (async () => {
    setLoadingIfFresh(marketplaceCatalog);
    try {
      marketplaceCatalog.value = { status: 'loaded', data: await fetchPluginCatalog() };
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

export async function installMarketplacePlugin(plugin: MarketplacePlugin): Promise<void> {
  try {
    const request = await stagePluginInstall(plugin.source);
    panelOverlay.value = { type: 'form', form: { type: 'plugin-install', request } };
    pushNavState();
    revealContentPane();
  } catch (e) {
    showToast(`Failed to stage plugin install: ${errorDetail(e)}`, 'error');
  }
}
