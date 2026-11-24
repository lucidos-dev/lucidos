import {
  pluginsInstalledOnly,
  setPluginsInstalledOnly,
  pluginsMarketplaceFilter,
  marketplaceCatalog,
  installedPlugins,
} from '../../store/store';
import { AppSearchBar } from '../apps/AppSearchBar';
import { Dropdown } from '../shared/Dropdown';
import type { DropdownOption } from '../shared/Dropdown';
import {
  StoreTab,
  openMarketplaceSettings,
  pluginRowsInScope,
  availableMarketplaces,
  resolveActiveMarketplace,
} from './StoreTab';
import { CatalogScanFailure } from './CatalogScanFailure';

const ALL_MARKETPLACES = 'filter:all';
const ADD_MARKETPLACE = 'action:add-marketplace';

export function marketplaceDropdownOptions(
  marketplaces: { id: string; name: string }[],
): DropdownOption[] {
  return [
    { value: ALL_MARKETPLACES, label: 'All marketplaces' },
    ...marketplaces.map((m) => ({ value: m.id, label: m.name })),
    { value: ADD_MARKETPLACE, label: 'Add marketplace…' },
  ];
}

export function PluginsView() {
  const installedOnly = pluginsInstalledOnly.value;

  const marketplaces = availableMarketplaces(
    pluginRowsInScope(marketplaceCatalog.value, installedPlugins.value, installedOnly),
  );
  const activeMarketplace = resolveActiveMarketplace(
    pluginsMarketplaceFilter.value,
    marketplaces,
  );

  return (
    <div class="content-view active apps-view plugins-view">
      <div class="plugins-filter-bar">
        <div class="segmented-control" role="group" aria-label="Filter plugins by install state">
          <button
            type="button"
            class={`segmented-btn ${!installedOnly ? 'active' : ''}`}
            onClick={() => setPluginsInstalledOnly(false)}
          >
            All
          </button>
          <button
            type="button"
            class={`segmented-btn ${installedOnly ? 'active' : ''}`}
            onClick={() => setPluginsInstalledOnly(true)}
          >
            Installed
          </button>
        </div>

        <Dropdown
          class="plugins-marketplace-filter"
          options={marketplaceDropdownOptions(marketplaces)}
          value={activeMarketplace?.id ?? ALL_MARKETPLACES}
          onChange={(value) => {
            if (value === ADD_MARKETPLACE) {
              openMarketplaceSettings();
              return;
            }
            pluginsMarketplaceFilter.value = value === ALL_MARKETPLACES ? null : value;
          }}
        />
      </div>

      <CatalogScanFailure />

      <AppSearchBar placeholder="Search plugins…" dataRole="plugins-search-input" />

      <StoreTab />
    </div>
  );
}
