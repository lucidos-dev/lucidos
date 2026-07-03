import { API, json } from './_core';
import type {
  InstalledPlugin,
  MarketplaceCatalog,
  PluginInstallRequest,
  PluginMarketplace,
  PluginUninstallRequest,
} from '../../store/types';

export interface AddMarketplaceResponse {
  marketplace: PluginMarketplace;
  marketplaces: PluginMarketplace[];
  created: boolean;
  commit: string;
}

export interface RemoveMarketplaceResponse {
  marketplaces: PluginMarketplace[];
  removed: boolean;
  commit: string;
}

export function addPluginMarketplace(
  source: string,
  name?: string,
): Promise<AddMarketplaceResponse> {
  return json(`${API}/plugins/marketplaces`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ source, name }),
  });
}

export function removePluginMarketplace(id: string): Promise<RemoveMarketplaceResponse> {
  return json(`${API}/plugins/marketplaces/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
}

export function fetchPluginCatalog(): Promise<MarketplaceCatalog> {
  // The catalog scan shallow-clones every registered marketplace repo, so it can
  // run well past the 10s default on a slow link — give it more headroom before
  // the client aborts (a premature timeout flips the Loadable to a spurious
  // "Failed to load plugin catalog"). The action layer retries a timeout anyway,
  // but a fair first attempt avoids the user-visible retry delay.
  return json(`${API}/plugins/catalog`, undefined, 30000);
}

/** Installed plugins from the event projection (no marketplace scan). Backs the
 *  Plugins → Installed tab. */
export function fetchInstalledPlugins(): Promise<{ plugins: InstalledPlugin[] }> {
  return json(`${API}/plugins/installed`);
}

export function stagePluginInstall(source: string): Promise<PluginInstallRequest> {
  return json(`${API}/plugins/install-request`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ source }),
  });
}

export function stagePluginUninstall(id: string): Promise<PluginUninstallRequest> {
  return json(`${API}/plugins/uninstall-request`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id }),
  });
}
