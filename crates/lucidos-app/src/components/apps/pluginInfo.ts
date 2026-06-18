import type { MarketplacePlugin } from '../../store/types';
import type { AppPluginInfo } from './AppCard';

/** Map installed app ids → their marketplace provenance + update status, from
 *  the catalog's plugins. Only installed/updatable plugins that ship an app
 *  contribute; locally-authored apps aren't in the result (no label). When the
 *  same app id appears in more than one marketplace, the entry reporting an
 *  available update wins so the user always sees the actionable one. */
export function resolvePluginInfo(plugins: MarketplacePlugin[]): Map<string, AppPluginInfo> {
  const map = new Map<string, AppPluginInfo>();
  for (const plugin of plugins) {
    if (!plugin.app_id) continue;
    if (plugin.status !== 'installed' && plugin.status !== 'update_available') continue;
    const updateAvailable = plugin.status === 'update_available';
    const existing = map.get(plugin.app_id);
    if (!existing || (updateAvailable && !existing.updateAvailable)) {
      map.set(plugin.app_id, { marketplaceName: plugin.marketplace_name, updateAvailable, plugin });
    }
  }
  return map;
}
