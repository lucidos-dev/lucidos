import { useEffect, useRef } from 'preact/hooks';
import { appsList, marketplaceCatalog, appsTab, appSearchOpen, appSearchQuery } from '../../store/store';
import type { MarketplacePlugin } from '../../store/types';
import {
  openApp,
  createNewApp,
  confirmDeleteApp,
  openEditApp,
  setAppsTab,
  closeAppSearch,
} from '../../store/actions/apps';
import { loadPluginCatalog, installMarketplacePlugin } from '../../store/actions/plugin-marketplaces';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { LoadableError } from '../shared/LoadableError';
import { SearchIcon, CloseIcon } from '../shared/icons';
import { AppRow, type AppPluginInfo } from './AppCard';
import { StoreTab } from './StoreTab';
import { resolvePluginInfo } from './pluginInfo';

/** Installed app id → marketplace provenance + update status, from the loaded
 *  catalog. Best-effort: empty until the catalog loads, so the Installed list
 *  never blocks on a marketplace scan. */
function pluginInfoByAppId(): Map<string, AppPluginInfo> {
  const cat = marketplaceCatalog.value;
  return cat.status === 'loaded' ? resolvePluginInfo(cat.data.plugins) : new Map();
}

function AppSearchBar() {
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => { inputRef.current?.focus({ preventScroll: true }); }, []);
  return (
    <div class="apps-search-bar">
      <SearchIcon className="apps-search-icon" />
      <input
        ref={inputRef}
        class="apps-search-input"
        data-role="apps-search-input"
        type="text"
        placeholder="Search apps..."
        value={appSearchQuery.value}
        onInput={(e) => { appSearchQuery.value = (e.currentTarget as HTMLInputElement).value; }}
        onKeyDown={(e) => { if (e.key === 'Escape') closeAppSearch(); }}
      />
      <button class="icon-btn header-icon" onClick={closeAppSearch} aria-label="Close search">
        <CloseIcon />
      </button>
    </div>
  );
}

function InstalledTab() {
  const loadable = appsList.value;
  const showLoading = useDelayedLoading(loadable);
  if (loadable.status === 'failed') {
    return (
      <div class="list-rows">
        <LoadableError noun="apps" error={loadable.error} />
      </div>
    );
  }
  if (loadable.status !== 'loaded') {
    if (!showLoading) return null;
    return (
      <div class="list-rows">
        <div class="loading-spinner" />
      </div>
    );
  }

  const pluginInfo = pluginInfoByAppId();
  const query = appSearchQuery.value.trim().toLowerCase();
  const apps = query
    ? loadable.data.filter((app) => {
        const marketplace = pluginInfo.get(app.id)?.marketplaceName ?? '';
        return (
          app.name.toLowerCase().includes(query) ||
          (app.description ?? '').toLowerCase().includes(query) ||
          marketplace.toLowerCase().includes(query)
        );
      })
    : loadable.data;

  if (query && apps.length === 0) {
    return <div class="empty-state"><p>No installed apps match "{appSearchQuery.value.trim()}".</p></div>;
  }

  const stageUpdate = (plugin: MarketplacePlugin) => void installMarketplacePlugin(plugin);

  return (
    <div class="list-rows">
      {apps.map((app) => {
        const info = pluginInfo.get(app.id);
        return (
          <AppRow
            key={app.id}
            app={app}
            pluginInfo={info}
            onUpdate={info?.updateAvailable ? () => stageUpdate(info.plugin) : undefined}
            onOpen={() => openApp(app)}
            onEdit={() => openEditApp(app.id)}
            onDelete={() => void confirmDeleteApp(app.id, app.name)}
          />
        );
      })}
      {!query && (
        <div class="list-row-add-card" onClick={createNewApp}>
          <div class="list-row-add-icon">+</div>
          <div class="list-row-add-label">New App</div>
        </div>
      )}
    </div>
  );
}

export function AppsView() {
  const tab = appsTab.value;

  // Load the plugin catalog once (cached) so the Installed tab can label apps
  // with their marketplace and surface update badges. The Store tab force-
  // refreshes it on its own mount; this non-forced load just primes the cache
  // and never blocks the installed list (which renders from appsList).
  useEffect(() => { void loadPluginCatalog(); }, []);

  return (
    <div class="content-view active apps-view">
      <div class="apps-tabs segmented-control">
        <button
          class={`segmented-btn${tab === 'installed' ? ' active' : ''}`}
          onClick={() => setAppsTab('installed')}
        >
          Installed
        </button>
        <button
          class={`segmented-btn${tab === 'store' ? ' active' : ''}`}
          onClick={() => setAppsTab('store')}
        >
          Store
        </button>
      </div>

      {appSearchOpen.value && <AppSearchBar />}

      {tab === 'installed' ? <InstalledTab /> : <StoreTab />}
    </div>
  );
}
