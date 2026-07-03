import { useEffect, useRef } from 'preact/hooks';
import { pluginsInstalledOnly, setPluginsInstalledOnly, appSearchOpen, appSearchQuery } from '../../store/store';
import { closeAppSearch } from '../../store/actions/apps';
import { SearchIcon, CloseIcon } from '../shared/icons';
import { StoreTab } from './StoreTab';

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

export function PluginsView() {
  // One unified list (StoreTab) lists every plugin the same way whether installed
  // or not — a status badge marks the installed ones, an Uninstall button removes
  // them. The All | Installed toggle just narrows that one list (default All);
  // the category pills and live search compose on top. (The former Installed |
  // Store tabs and the separate installed-only list are gone.)
  const installedOnly = pluginsInstalledOnly.value;

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
      </div>

      {appSearchOpen.value && <PluginSearchBar />}

      <StoreTab />
    </div>
  );
}
