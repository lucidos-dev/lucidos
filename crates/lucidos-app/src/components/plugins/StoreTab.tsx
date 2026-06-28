import { useEffect, useState } from 'preact/hooks';
import {
  marketplaceCatalog,
  installedPlugins,
  appSearchQuery,
  pluginsInstalledOnly,
  setPluginsInstalledOnly,
  pluginScrollTarget,
} from '../../store/store';
import type { InstalledPlugin, MarketplacePlugin } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeleton } from '../shared/ListSkeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { installMarketplacePlugin, refreshPluginCatalog } from '../../store/actions/plugin-marketplaces';
import { loadInstalledPlugins } from '../../store/actions/plugins';
import { openAppById } from '../../store/actions/apps';
import { focusThread } from '../../store/actions/threads';
import { uninstallMarketplacePlugin } from '../../store/actions/plugin-uninstall';
import { openSettingsSubview, switchMenuItem } from '../../store/actions/menu';
import { AddOfficialMarketplaceButton } from './AddOfficialMarketplaceButton';
import { contentLabel } from './pluginContent';

/** Jump to Settings → Marketplaces from anywhere. `switchMenuItem` sets the
 *  Settings panel as the active menu item (resetting the subview to main);
 *  `openSettingsSubview` then lands on Marketplaces — the same two-step the
 *  navigate_ui settings deep-link uses. */
function openMarketplaceSettings() {
  switchMenuItem('settings');
  openSettingsSubview('marketplaces');
}

const CATALOG_REFRESH_MS = 5 * 60 * 1000;
const HIGHLIGHT_CLASS = 'plugin-row-highlight';

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

/** Title-case a kebab-case category id for display: `developer-tools` →
 *  `Developer tools`. */
function categoryLabel(category: string): string {
  const spaced = category.replace(/-/g, ' ');
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

/** An installed plugin whose marketplace is no longer registered won't appear in
 *  the marketplace catalog scan — synthesize a catalog row for it so it still
 *  lists (and stays uninstallable) under both All and Installed. It carries no
 *  marketplace metadata (no description / categories), so it renders with just
 *  its name, "Installed vX" badge, content chips, file count, and Uninstall. */
function orphanRow(p: InstalledPlugin): MarketplacePlugin {
  return {
    marketplace_id: `installed:${p.id}`,
    marketplace_name: p.source ?? '',
    id: p.id,
    name: p.name,
    description: '',
    version: p.version,
    source: p.source ?? '',
    manifest: {},
    content: p.content,
    categories: [],
    files_count: p.files.length,
    status: 'installed',
    installed_version: p.version,
    app_id: p.app_id,
  };
}

/** The unified row set. All → the whole catalog plus any installed plugin the
 *  catalog scan missed (orphan). Installed → driven off the installed projection
 *  (so the view survives a catalog-scan failure), each row enriched from the
 *  catalog where present (gives the update_available status + Update action +
 *  description/categories) and synthesized otherwise. */
function buildRows(
  catalog: MarketplacePlugin[],
  installed: InstalledPlugin[],
  installedOnly: boolean,
): MarketplacePlugin[] {
  if (installedOnly) {
    const catalogInstalledById = new Map<string, MarketplacePlugin>();
    for (const p of catalog) {
      if (p.status !== 'available' && !catalogInstalledById.has(p.id)) {
        catalogInstalledById.set(p.id, p);
      }
    }
    return installed.map((p) => catalogInstalledById.get(p.id) ?? orphanRow(p));
  }
  const catalogIds = new Set(catalog.map((p) => p.id));
  const orphans = installed.filter((p) => !catalogIds.has(p.id)).map(orphanRow);
  return [...catalog, ...orphans];
}

export function StoreTab() {
  const installedOnly = pluginsInstalledOnly.value;
  const catLoadable = marketplaceCatalog.value;
  const instLoadable = installedPlugins.value;
  // The source the loading/failed gating keys on flips with the mode: Installed
  // renders from the installed projection (catalog best-effort, for the update
  // status), All renders from the catalog (installed list best-effort, for
  // orphan coverage). Both are fetched on mount, so flipping the toggle never
  // re-flashes the spinner.
  const primary = installedOnly ? instLoadable : catLoadable;
  const showLoading = useDelayedLoading(primary);
  const [installingSource, setInstallingSource] = useState<string | null>(null);
  const [selectedCategory, setSelectedCategory] = useState<string | null>(null);

  useEffect(() => {
    // Refresh both on mount (= each time the panel opens). The catalog gives
    // status + categories; the installed list gives orphan coverage and drives
    // the Installed view. refreshPluginCatalog only shows the spinner when the
    // catalog is still fresh, so a revisit re-fetch doesn't flash loading.
    void refreshPluginCatalog();
    void loadInstalledPlugins();
    const id = window.setInterval(() => {
      if (marketplaceCatalog.value.status === 'loaded') void refreshPluginCatalog();
    }, CATALOG_REFRESH_MS);
    return () => window.clearInterval(id);
  }, []);

  // Notification deep-link (navigate_ui target `plugins`): once the list has
  // rendered, scroll the targeted plugin's row into view and pulse it. The
  // target is consumed only on a successful scroll OR when the row is genuinely
  // absent with no search filter active — so a leftover search that hides the row
  // doesn't swallow the deep-link; clearing the filter re-runs this
  // (appSearchQuery is a dep) and the scroll then lands.
  useEffect(() => {
    const target = pluginScrollTarget.value;
    if (!target || primary.status !== 'loaded') return;
    const el = document.querySelector<HTMLElement>(`[data-plugin-id="${CSS.escape(target)}"]`);
    if (el) {
      el.scrollIntoView({ block: 'center', behavior: 'smooth' });
      el.classList.remove(HIGHLIGHT_CLASS);
      void el.offsetWidth; // force reflow so the pulse re-triggers on repeat taps
      el.classList.add(HIGHLIGHT_CLASS);
      pluginScrollTarget.value = null;
    } else if (!appSearchQuery.value.trim()) {
      // Row not in the list and nothing is filtering it out → the plugin is
      // genuinely gone (uninstalled / stale target). Give up so it can't linger.
      pluginScrollTarget.value = null;
    }
  }, [pluginScrollTarget.value, primary.status, appSearchQuery.value, installedOnly]);

  async function stageInstall(plugin: MarketplacePlugin) {
    setInstallingSource(plugin.source);
    try {
      await installMarketplacePlugin(plugin);
    } finally {
      setInstallingSource(null);
    }
  }

  if (primary.status === 'failed') {
    return (
      <div class="list-rows">
        <LoadableError noun={installedOnly ? 'installed plugins' : 'plugin catalog'} error={primary.error} />
      </div>
    );
  }

  return (
    <div class="list-rows">
      <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeleton />}>
        {primary.status === 'loaded' ? (
          <StoreTabLoaded
            installedOnly={installedOnly}
            installingSource={installingSource}
            selectedCategory={selectedCategory}
            setSelectedCategory={setSelectedCategory}
            stageInstall={stageInstall}
          />
        ) : null}
      </LoadingFade>
    </div>
  );
}

function StoreTabLoaded({
  installedOnly,
  installingSource,
  selectedCategory,
  setSelectedCategory,
  stageInstall,
}: {
  installedOnly: boolean;
  installingSource: string | null;
  selectedCategory: string | null;
  setSelectedCategory: (c: string | null) => void;
  stageInstall: (plugin: MarketplacePlugin) => void;
}) {
  const catLoadable = marketplaceCatalog.value;
  const instLoadable = installedPlugins.value;

  const catalog = catLoadable.status === 'loaded' ? catLoadable.data : null;
  const installed = instLoadable.status === 'loaded' ? instLoadable.data : [];
  const hasMarketplaces = (catalog?.marketplaces.length ?? 0) > 0;
  const query = appSearchQuery.value.trim().toLowerCase();

  // No marketplaces AND nothing installed → the onboarding suggestion. (With
  // installed orphans we fall through to the list so they stay visible and
  // uninstallable even after their marketplace is gone.) Only in All mode —
  // Installed always renders from the installed projection.
  if (!installedOnly && !hasMarketplaces && installed.length === 0) {
    return (
      <div class="empty-state app-store-empty">
        <div class="app-store-empty-suggest">
          <p>Add a marketplace to discover and install plugins.</p>
          <AddOfficialMarketplaceButton />
          <p class="app-store-empty-alt">
            or{' '}
            <button type="button" class="accent-link" onClick={openMarketplaceSettings}>
              register your own marketplace
            </button>
            .
          </p>
        </div>
      </div>
    );
  }

  const rows = buildRows(catalog?.plugins ?? [], installed, installedOnly);

  // Category filter — derived from the rows actually in scope (after the
  // install-state toggle) so a category with nothing under the current toggle
  // isn't offered. A stale selection (rows changed) falls back to "All" so the
  // list never silently shows nothing.
  const availableCategories = Array.from(new Set(rows.flatMap((p) => p.categories))).sort();
  const activeCategory =
    selectedCategory && availableCategories.includes(selectedCategory) ? selectedCategory : null;

  const plugins = rows.filter(
    (p) => matchesQuery(p, query) && (!activeCategory || p.categories.includes(activeCategory)),
  );

  return (
    <div class="app-store">
      {availableCategories.length > 0 && (
        <div class="app-store-category-filter" role="group" aria-label="Filter by category">
          <button
            type="button"
            class={`app-store-category-pill${!activeCategory ? ' active' : ''}`}
            onClick={() => setSelectedCategory(null)}
          >
            All
          </button>
          {availableCategories.map((c) => (
            <button
              type="button"
              key={c}
              class={`app-store-category-pill${activeCategory === c ? ' active' : ''}`}
              onClick={() => setSelectedCategory(activeCategory === c ? null : c)}
            >
              {categoryLabel(c)}
            </button>
          ))}
        </div>
      )}

      {plugins.length === 0 ? (
        <div class="empty-state">
          {query ? (
            <p>No plugins match "{appSearchQuery.value.trim()}".</p>
          ) : activeCategory ? (
            <p>No plugins in {categoryLabel(activeCategory)}.</p>
          ) : installedOnly ? (
            <p>
              No plugins installed yet. Browse the{' '}
              <button type="button" class="accent-link" onClick={() => setPluginsInstalledOnly(false)}>
                catalog
              </button>{' '}
              to install one.
            </p>
          ) : (
            <p>No plugins found.</p>
          )}
        </div>
      ) : (
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
            // focusThread (not …OrBootstrap): the setup thread may be spawned but
            // not yet materialized as a thread_summaries row (queued in the
            // Thread Queue), so a bootstrap fetch would 404 → "Thread not found".
            // focusThread sets focus and lets the row + events stream in over
            // SSE — same rationale as the confirm-navigation path in
            // plugin-install.ts. The catalog only surfaces this button for a
            // present-or-queued setup thread (a gone one resolves to Open).
            onPrimary = () => focusThread(action.threadId);
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
          <div
            class="list-row app-store-plugin-row"
            data-plugin-id={plugin.id}
            key={`${plugin.marketplace_id}-${plugin.id}`}
          >
            <div class="list-row-info">
              <div class="app-store-plugin-title-row">
                <span class="title list-row-name">{plugin.name}</span>
                <span class={`app-store-status app-store-status-${plugin.status}`}>
                  {statusLabel(plugin)}
                </span>
              </div>
              {plugin.description && <div class="list-row-details">{plugin.description}</div>}
              <div class="app-store-plugin-meta">
                {plugin.marketplace_name && <span>{plugin.marketplace_name}</span>}
                <span>{fileCountLabel(plugin.files_count)}</span>
                {plugin.content.map((kind) => (
                  <span class="app-store-content-chip" key={kind}>
                    {contentLabel(kind)}
                  </span>
                ))}
                {plugin.categories.map((c) => (
                  <span class="app-store-category-chip" key={`cat-${c}`}>
                    {categoryLabel(c)}
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
      )}
    </div>
  );
}
