import type { App, MarketplacePlugin } from '../../store/types';
import { isAppPinned, togglePinApp } from '../../store/actions/pinnedApps';
import { PinIcon } from '../shared/icons';
import { useSkeleton, SkText, SkBlock } from '../shared/Skeleton';

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

/** Self-skeletonizing row: rendered with no props inside a SkeletonProvider
 *  (`<AppRow />`) it draws itself as a loading placeholder via the Sk* leaves;
 *  with real props it renders normally. Props are optional only to support the
 *  skeleton call; real call sites pass them all. */
export function AppRow({ app, onOpen, onEdit, onDelete, pluginInfo, onUpdate }: Partial<AppRowProps>) {
  const sk = useSkeleton();
  const pinned = app ? isAppPinned(app.id) : false;
  // One label for both the tooltip and the accessible name: the pin is
  // icon-only, so a sighted user and a screen-reader user must be told the
  // same thing about what it does. It names the ACTION (what the click will
  // do), which is why there's no `aria-pressed` alongside it: a toggle-state
  // attribute paired with an action name announces "Unpin from the menu,
  // pressed", which reads as the opposite of the truth.
  const pinLabel = pinned ? 'Unpin from the menu' : 'Pin to the menu';
  return (
    <div class={`list-row app-row${sk ? '' : ' clickable'}`} onClick={sk ? undefined : onOpen}>
      <div class="list-row-info">
        <div class="app-row-title-line">
          <SkText class="title list-row-name" w="9rem">{app?.name}</SkText>
          {pluginInfo && (
            <span class="app-marketplace-chip" data-tooltip={`Installed from ${pluginInfo.marketplaceName}`}>
              {pluginInfo.marketplaceName}
            </span>
          )}
          {pluginInfo?.updateAvailable && (
            <span class="app-update-chip">Update available</span>
          )}
        </div>
        {(sk || app?.description) && (
          <SkText class="list-row-details" as="div" w="16rem">{app?.description}</SkText>
        )}
      </div>
      <div class="list-row-actions">
        {pluginInfo?.updateAvailable && onUpdate && (
          <button class="action-btn" onClick={(e) => { e.stopPropagation(); onUpdate(); }}>Update</button>
        )}
        <SkBlock w="2rem" h="2rem" round>
          <button
            class={`icon-btn ${pinned ? 'pinned' : ''}`}
            onClick={(e) => { e.stopPropagation(); if (app) togglePinApp(app.id); }}
            aria-label={pinLabel}
            data-tooltip={pinLabel}
          >
            <PinIcon filled={pinned} />
          </button>
        </SkBlock>
        <SkBlock w="3rem" h="2rem" round>
          <button class="action-btn" onClick={(e) => { e.stopPropagation(); onEdit?.(); }}>Edit</button>
        </SkBlock>
        <SkBlock w="3.75rem" h="2rem" round>
          <button class="action-btn action-btn-danger" onClick={(e) => { e.stopPropagation(); onDelete?.(); }}>Delete</button>
        </SkBlock>
      </div>
    </div>
  );
}
