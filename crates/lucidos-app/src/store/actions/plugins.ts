import { installedPlugins, panelOverlay } from '../store';
import { setLoadingIfFresh, toFailed } from '../types';
import type { MarketplacePlugin } from '../types';
import { revealContentPane } from './pane';
import { pushNavState } from './navigation';
import { fetchInstalledPlugins } from '../../api/client';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';

let loadSeq = 0;

/** Load the installed-plugins list (Plugins panel). Re-fetches on each
 *  call so the list reflects installs/uninstalls; `setLoadingIfFresh` keeps a
 *  revisit from flashing the spinner once the list is already loaded.
 *
 *  Only the newest call applies its result. Joining an in-flight call instead
 *  would let a reload after an install settle on a read from before it. */
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

/** Open the *plugin detail page* for one Plugins panel row. */
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
