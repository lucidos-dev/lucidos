import { useEffect, useRef } from 'preact/hooks';
import { appsList, marketplaceCatalog, appSearchOpen, appSearchQuery } from '../../store/store';
import type { MarketplacePlugin } from '../../store/types';
import {
  openApp,
  createNewApp,
  confirmDeleteApp,
  openEditApp,
  closeAppSearch,
  refreshApps,
} from '../../store/actions/apps';
import { loadPluginCatalog, refreshPluginCatalog } from '../../store/actions/plugin-marketplaces';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { installMarketplacePlugin } from '../../store/actions/plugin-install';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { LoadableError } from '../shared/LoadableError';
import { ListRowAddCard } from '../shared/ListRowAddCard';
import { ListSkeletonOf } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { CloseIcon } from '../shared/icons';
import { SearchField } from '../shared/SearchField';
import { AppRow, type AppPluginInfo } from './AppCard';
import { resolvePluginInfo } from './pluginInfo';

/** Installed app id → marketplace provenance + update status, from the loaded
 *  catalog. Best-effort: empty until the catalog loads, so the app list never
 *  blocks on a marketplace scan. */
function pluginInfoByAppId(): Map<string, AppPluginInfo> {
  const cat = marketplaceCatalog.value;
  return cat.status === 'loaded' ? resolvePluginInfo(cat.data.plugins) : new Map();
}

function AppSearchBar() {
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => { inputRef.current?.focus({ preventScroll: true }); }, []);
  return (
    <div class="apps-search-bar">
      <SearchField
        class="apps-search-field"
        inputRef={inputRef}
        data-role="apps-search-input"
        placeholder="Search apps…"
        value={appSearchQuery.value}
        onInput={(e) => { appSearchQuery.value = e.currentTarget.value; }}
        onKeyDown={(e) => { if (e.key === 'Escape') closeAppSearch(); }}
      />
      <button class="icon-btn header-icon" onClick={closeAppSearch} aria-label="Close search">
        <CloseIcon />
      </button>
    </div>
  );
}

/** The apps list, and the catalog its rows read their plugin labels from. */
function refreshAppsPanel(): Promise<unknown> {
  return Promise.all([
    refreshApps(),
    refreshPluginCatalog(),
  ]);
}

export function AppsView() {
  usePanelRefresh('apps', refreshAppsPanel);
  const loadable = appsList.value;
  const showLoading = useDelayedLoading(loadable);

  // Prime the plugin catalog once (cached) so app rows can label their
  // marketplace provenance and surface an Update badge. Never blocks the app
  // list — that renders from appsList regardless.
  useEffect(() => { void loadPluginCatalog(); }, []);

  let body;
  if (loadable.status === 'failed') {
    body = <LoadableError noun="apps" error={loadable.error} />;
  } else {
    body = (
      <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf fill containerClass="list-rows" row={() => <AppRow />} />}>
        {loadable.status === 'loaded'
          ? (() => {
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
                  {!query && <ListRowAddCard label="New App" onClick={createNewApp} />}
                </div>
              );
            })()
          : null}
      </LoadingFade>
    );
  }

  return (
    <div class="content-view active apps-view">
      {appSearchOpen.value && <AppSearchBar />}
      {body}
    </div>
  );
}
