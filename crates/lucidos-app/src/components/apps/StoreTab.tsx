import { useEffect, useState } from 'preact/hooks';
import { marketplaceCatalog, appSearchQuery } from '../../store/store';
import type { MarketplacePlugin } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { LoadableError } from '../shared/LoadableError';
import { installMarketplacePlugin, refreshPluginCatalog } from '../../store/actions/plugin-marketplaces';
import { openAppById } from '../../store/actions/apps';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { uninstallMarketplacePlugin } from '../../store/actions/plugin-uninstall';
import { openSettingsSubview, switchMenuItem } from '../../store/actions/menu';

/** Jump to Settings → Marketplaces from anywhere. `switchMenuItem` sets the
 *  Settings panel as the active menu item (resetting the subview to main);
 *  `openSettingsSubview` then lands on Marketplaces — the same two-step the
 *  navigate_ui settings deep-link uses. */
function openMarketplaceSettings() {
  switchMenuItem('settings');
  openSettingsSubview('marketplaces');
}

const CONTENT_LABELS: Record<string, string> = {
  apps: 'Apps',
  knowhow: 'Knowhow',
  triggers: 'Triggers',
  scripts: 'Scripts',
  'auth-modules': 'Auth',
};

const CATALOG_REFRESH_MS = 5 * 60 * 1000;

function statusLabel(plugin: MarketplacePlugin): string {
  switch (plugin.status) {
    case 'installed': return `Installed v${plugin.installed_version ?? plugin.version}`;
    case 'update_available': return `Update from v${plugin.installed_version} to v${plugin.version}`;
    case 'available': return `v${plugin.version}`;
  }
}

function actionLabel(plugin: MarketplacePlugin): string {
  return plugin.status === 'update_available' ? 'Update' : 'Install';
}

/** The card's primary button. Progresses Install/Update → Setup → Open:
 *  - not installed → Install (or Update for an out-of-date install)
 *  - installed with an unfinished setup thread → Setup (opens that thread)
 *  - installed and setup done (or none) with an app → Open (launches it)
 *  - installed with nothing to open → a disabled "Installed" label
 *  An out-of-date install always shows Update first, before Setup/Open. */
type CardAction =
  | { kind: 'install'; label: string }
  | { kind: 'setup'; threadId: string }
  | { kind: 'open'; appId: string }
  | { kind: 'none' };

function cardPrimaryAction(plugin: MarketplacePlugin): CardAction {
  if (plugin.status !== 'installed') return { kind: 'install', label: actionLabel(plugin) };
  if (plugin.setup_thread_id && !plugin.setup_complete) {
    return { kind: 'setup', threadId: plugin.setup_thread_id };
  }
  if (plugin.app_id) return { kind: 'open', appId: plugin.app_id };
  return { kind: 'none' };
}

function fileCountLabel(count: number): string {
  return `${count} ${count === 1 ? 'file' : 'files'}`;
}

function matchesQuery(plugin: MarketplacePlugin, query: string): boolean {
  if (!query) return true;
  return (
    plugin.name.toLowerCase().includes(query) ||
    plugin.description.toLowerCase().includes(query) ||
    plugin.marketplace_name.toLowerCase().includes(query)
  );
}

export function StoreTab() {
  const loadable = marketplaceCatalog.value;
  const showLoading = useDelayedLoading(loadable);
  const [installingSource, setInstallingSource] = useState<string | null>(null);

  useEffect(() => {
    // Refresh on every mount (= each time the user opens the Store tab) so a
    // card's content + Setup→Open state reflect anything that changed since
    // they last looked. refreshPluginCatalog only shows the spinner when the
    // catalog is still fresh, so a revisit re-fetch doesn't flash loading.
    void refreshPluginCatalog();
    const id = window.setInterval(() => {
      if (marketplaceCatalog.value.status === 'loaded') void refreshPluginCatalog();
    }, CATALOG_REFRESH_MS);
    return () => window.clearInterval(id);
  }, []);

  async function stageInstall(plugin: MarketplacePlugin) {
    setInstallingSource(plugin.source);
    try {
      await installMarketplacePlugin(plugin);
    } finally {
      setInstallingSource(null);
    }
  }

  if (loadable.status === 'failed') {
    return (
      <div class="list-rows">
        <LoadableError noun="plugin catalog" error={loadable.error} />
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

  const catalog = loadable.data;
  const hasMarketplaces = catalog.marketplaces.length > 0;
  const query = appSearchQuery.value.trim().toLowerCase();
  const plugins = catalog.plugins.filter((p) => matchesQuery(p, query));

  if (plugins.length === 0) {
    return (
      <div class="empty-state app-store-empty">
        {!hasMarketplaces ? (
          <p>
            <button
              type="button"
              class="accent-link"
              onClick={openMarketplaceSettings}
            >
              Register a marketplace
            </button>
            {' '}to load plugins.
          </p>
        ) : query ? (
          <p>No plugins match "{appSearchQuery.value.trim()}".</p>
        ) : (
          <p>No plugins found.</p>
        )}
      </div>
    );
  }

  return (
    <div class="list-rows app-store-plugins">
      {plugins.map((plugin) => {
        // 'available' is the only not-yet-installed state; 'installed' and
        // 'update_available' both mean the plugin is on disk → uninstallable.
        const isInstalled = plugin.status !== 'available';
        const busy = installingSource === plugin.source;
        const action = cardPrimaryAction(plugin);
        let label: string;
        let onPrimary: (() => void) | undefined;
        let primaryDisabled = busy;
        switch (action.kind) {
          case 'install':
            label = action.label;
            onPrimary = () => void stageInstall(plugin);
            break;
          case 'setup':
            label = 'Setup';
            onPrimary = () => focusThreadOrBootstrap(action.threadId);
            break;
          case 'open':
            label = 'Open';
            onPrimary = () => void openAppById(action.appId);
            break;
          case 'none':
            label = 'Installed';
            primaryDisabled = true;
            break;
        }
        return (
          <div class="list-row app-store-plugin-row" key={`${plugin.marketplace_id}-${plugin.id}`}>
            <div class="list-row-info">
              <div class="app-store-plugin-title-row">
                <span class="title list-row-name">{plugin.name}</span>
                <span class={`app-store-status app-store-status-${plugin.status}`}>
                  {statusLabel(plugin)}
                </span>
              </div>
              <div class="list-row-details">{plugin.description}</div>
              <div class="app-store-plugin-meta">
                <span>{plugin.marketplace_name}</span>
                <span>{fileCountLabel(plugin.files_count)}</span>
                {plugin.content.map((kind) => (
                  <span class="app-store-content-chip" key={kind}>
                    {CONTENT_LABELS[kind] ?? kind}
                  </span>
                ))}
              </div>
            </div>
            <div class="list-row-actions">
              {isInstalled && (
                <button
                  class="action-btn action-btn-danger"
                  type="button"
                  onClick={() => void uninstallMarketplacePlugin(plugin)}
                >
                  Uninstall
                </button>
              )}
              <button
                class="action-btn"
                type="button"
                disabled={primaryDisabled}
                onClick={onPrimary}
              >
                {busy ? 'Staging' : label}
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}
