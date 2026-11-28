import { API, API_BASE, json, text } from './_core';
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
  return json(`${API}/plugins/catalog`);
}

export function rescanPluginCatalog(): Promise<{ queued: boolean }> {
  return json(`${API}/plugins/catalog/rescan`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: '{}',
  });
}

export function fetchInstalledPlugins(): Promise<{ plugins: InstalledPlugin[] }> {
  return json(`${API}/plugins/installed`);
}

export function pluginMediaUrl(url: string): string {
  return `${API_BASE}${url}`;
}

export function fetchPluginReadme(readmeUrl: string): Promise<string> {
  return text(pluginMediaUrl(readmeUrl));
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
