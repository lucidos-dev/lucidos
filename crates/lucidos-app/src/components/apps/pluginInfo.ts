import type { MarketplacePlugin } from '../../store/types';
import type { AppPluginInfo } from './AppCard';

/** Map installed app ids → their marketplace provenance + update status, from
 *  the catalog's plugins. Only installed/updatable plugins that ship an app
 *  contribute; locally-authored apps aren't in the result (no label). An update
 *  this Lucidos cannot install is not offered: it carries the engine's reason
 *  instead. When the same app id appears in more than one marketplace, the
 *  most actionable entry wins: an installable update, then a blocked one. */
export function resolvePluginInfo(plugins: MarketplacePlugin[]): Map<string, AppPluginInfo> {
  const map = new Map<string, AppPluginInfo>();
  for (const plugin of plugins) {
    if (!plugin.app_id) continue;
    if (plugin.status !== 'installed' && plugin.status !== 'update_available') continue;
    const newer = plugin.status === 'update_available';
    const info: AppPluginInfo = {
      marketplaceName: plugin.marketplace_name,
      updateAvailable: newer && plugin.engine_compatible,
      updateBlockedReason: newer && !plugin.engine_compatible ? plugin.engine_incompatible_reason : undefined,
      plugin,
    };
    const existing = map.get(plugin.app_id);
    if (!existing || rank(info) > rank(existing)) map.set(plugin.app_id, info);
  }
  return map;
}

function rank(info: AppPluginInfo): number {
  if (info.updateAvailable) return 2;
  return info.updateBlockedReason ? 1 : 0;
}
