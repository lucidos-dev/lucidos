import { useEffect, useRef } from 'preact/hooks';
import {
  pluginsInstalledOnly,
  setPluginsInstalledOnly,
  pluginsMarketplaceFilter,
  appSearchOpen,
  appSearchQuery,
  marketplaceCatalog,
  installedPlugins,
} from '../../store/store';
import { closeAppSearch } from '../../store/actions/apps';
import { SearchIcon, CloseIcon } from '../shared/icons';
import { Dropdown } from '../shared/Dropdown';
import type { DropdownOption } from '../shared/Dropdown';
import {
  StoreTab,
  openMarketplaceSettings,
  pluginRowsInScope,
  availableMarketplaces,
  resolveActiveMarketplace,
} from './StoreTab';

function PluginSearchBar() {
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => { inputRef.current?.focus({ preventScroll: true }); }, []);
  return (
    <div class="apps-search-bar">
      <SearchIcon className="apps-search-icon" />
      <input
        ref={inputRef}
        class="apps-search-input"
        data-role="plugins-search-input"
        type="text"
        placeholder="Search plugins..."
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

/** Sentinels for the two dropdown rows that are not a marketplace. Both carry a
 *  colon. The engine builds a marketplace id out of ASCII alphanumerics and
 *  dashes alone, so neither can collide with a real one. */
const ALL_MARKETPLACES = 'filter:all';
const ADD_MARKETPLACE = 'action:add-marketplace';

/** The dropdown's rows: All, then each marketplace, then the Add shortcut last.
 *  The panel offers it whatever the count, one marketplace or none. The Add
 *  row is the panel's only always-present way to register a marketplace. Hiding
 *  the control to save a redundant filter would take that with it. Pure +
 *  exported for the unit test. */
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
  // One unified list (StoreTab) lists every plugin the same way whether installed
  // or not — a status badge marks the installed ones, an Uninstall button removes
  // them. The All | Installed toggle just narrows that one list (default All);
  // the category pills and live search compose on top. (The former Installed |
  // Store tabs and the separate installed-only list are gone.)
  const installedOnly = pluginsInstalledOnly.value;

  // The dropdown lives here because it sits in the filter bar, while the list it
  // narrows is a sibling below. Both derive the rows from the same signals, so
  // both reach the same answer without either owning the other.
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
            // The Add row navigates instead of filtering, so it must not become
            // the selection: leave the picked marketplace exactly as it was.
            if (value === ADD_MARKETPLACE) {
              openMarketplaceSettings();
              return;
            }
            pluginsMarketplaceFilter.value = value === ALL_MARKETPLACES ? null : value;
          }}
        />
      </div>

      {appSearchOpen.value && <PluginSearchBar />}

      <StoreTab />
    </div>
  );
}
