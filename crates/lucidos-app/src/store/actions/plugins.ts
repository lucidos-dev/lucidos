import { installedPlugins, panelOverlay } from '../store';
import { setLoadingIfFresh, toFailed } from '../types';
import type { MarketplacePlugin } from '../types';
import { revealContentPane } from './pane';
import { pushNavState } from './navigation';
import { fetchInstalledPlugins } from '../../api/client';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';

let loadSeq = 0;

export const installedPluginsSlice: SliceOf<'installed_plugins', number> = {
  begin: () => {
    const seq = ++loadSeq;
    setLoadingIfFresh(installedPlugins);
    return seq;
  },
  apply: ({ plugins }, seq) => {
    if (seq !== loadSeq) return;
    installedPlugins.value = { status: 'loaded', data: plugins };
  },
  fail: (e, seq) => {
    if (seq !== loadSeq) return;
    installedPlugins.value = toFailed(e);
  },
};

export function loadInstalledPlugins(): Promise<void> {
  return loadSlice('installed_plugins', installedPluginsSlice, () => fetchInstalledPlugins());
}

export function openPluginDetail(plugin: MarketplacePlugin): void {
  panelOverlay.value = {
    type: 'plugin-detail',
    marketplaceId: plugin.marketplace_id,
    pluginId: plugin.id,
    name: plugin.name,
  };
  revealContentPane();
  pushNavState();
}
