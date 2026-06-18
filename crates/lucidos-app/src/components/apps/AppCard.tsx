import type { App, MarketplacePlugin } from '../../store/types';
import { isAppPinned, togglePinApp } from '../../store/actions/pinnedApps';
import { PinIcon } from '../shared/icons';

/** Marketplace provenance for an installed app, resolved from the plugin
 *  catalog: which marketplace it came from and whether a newer version is
 *  available. Absent for apps the user authored locally. */
export interface AppPluginInfo {
  marketplaceName: string;
  updateAvailable: boolean;
  plugin: MarketplacePlugin;
}

interface AppRowProps {
  app: App;
  onOpen: () => void;
  onEdit: () => void;
  onDelete: () => void;
  /** Set when this app was installed from a plugin marketplace. */
  pluginInfo?: AppPluginInfo;
  /** Stage the plugin update (shown only when pluginInfo.updateAvailable). */
  onUpdate?: () => void;
}

export function AppRow({ app, onOpen, onEdit, onDelete, pluginInfo, onUpdate }: AppRowProps) {
  const pinned = isAppPinned(app.id);
  return (
    <div class="list-row app-row clickable" onClick={onOpen}>
      <div class="list-row-info">
        <div class="app-row-title-line">
          <span class="title list-row-name">{app.name}</span>
          {pluginInfo && (
            <span class="app-marketplace-chip" data-tooltip={`Installed from ${pluginInfo.marketplaceName}`}>
              {pluginInfo.marketplaceName}
            </span>
          )}
          {pluginInfo?.updateAvailable && (
            <span class="app-update-chip">Update available</span>
          )}
        </div>
        {app.description && (
          <div class="list-row-details">{app.description}</div>
        )}
      </div>
      <div class="list-row-actions">
        {pluginInfo?.updateAvailable && onUpdate && (
          <button class="action-btn" onClick={(e) => { e.stopPropagation(); onUpdate(); }}>Update</button>
        )}
        <button
          class={`icon-btn ${pinned ? 'pinned' : ''}`}
          onClick={(e) => { e.stopPropagation(); togglePinApp(app.id); }}
          aria-label={pinned ? 'Unpin from menu' : 'Pin to menu'}
        >
          <PinIcon filled={pinned} />
        </button>
        <button class="action-btn" onClick={(e) => { e.stopPropagation(); onEdit(); }}>Edit</button>
        <button class="action-btn action-btn-danger" onClick={(e) => { e.stopPropagation(); onDelete(); }}>Delete</button>
      </div>
    </div>
  );
}
