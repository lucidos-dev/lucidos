import { API, json } from './_core';
import type {
  MarketplaceCatalog,
  PluginInstallRequest,
  PluginMarketplace,
  PluginUninstallRequest,
} from '../../store/types';

export interface MarketplacesResponse {
  marketplaces: PluginMarketplace[];
}

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

export function listPluginMarketplaces(): Promise<MarketplacesResponse> {
  return json(`${API}/plugins/marketplaces`);
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
