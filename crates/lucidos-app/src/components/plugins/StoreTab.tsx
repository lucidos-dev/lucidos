import { useEffect, useState } from 'preact/hooks';
import {
  marketplaceCatalog,
  marketplaceScanning,
  installedPlugins,
  appSearchQuery,
  pluginsInstalledOnly,
  setPluginsInstalledOnly,
  pluginsMarketplaceFilter,
  pluginScrollTarget,
} from '../../store/store';
import type { InstalledPlugin, Loadable, MarketplacePlugin } from '../../store/types';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf, useSkeleton, SkText, SkeletonProvider } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { refreshPluginCatalog, rescanPluginCatalogAndSettle } from '../../store/actions/plugin-marketplaces';
import { installMarketplacePlugin } from '../../store/actions/plugin-install';
import { loadInstalledPlugins, openPluginDetail } from '../../store/actions/plugins';
import { categoryLabel, PluginActions, PluginIcon, PluginMeta, PluginTitle } from './pluginCard';
import { openSettingsSubview } from '../../store/actions/menu';
import { AddOfficialMarketplaceButton } from './AddOfficialMarketplaceButton';
import { applyNavFocus } from '../shared/focusMarker';
import { scrollBehavior } from '../../utils/motion';

/** Jump to Settings → Marketplaces from anywhere. One call: `openSettingsSubview`
 *  lands the Settings panel and the sub-section together, so the jump is a single
 *  nav-history entry (the same shape the navigate_ui settings deep link uses). */
export function openMarketplaceSettings() {
  openSettingsSubview('marketplaces');
}

/** Widths of the placeholder pills in the loading skeleton, one per label of a
 *  FULLY-populated category bar (`All` + the ~9 controlled-vocabulary categories,
 *  see `core/plugins.rs` PLUGIN_CATEGORIES). The skeleton uses the real bar's
 *  class, so it scrolls on touch and wraps on a fine pointer exactly as the
 *  real bar does. */
const SKELETON_PILL_WIDTHS = [
  '1rem', // All
  '5.25rem', // Productivity
  '3rem', // Finance
  '2.5rem', // Health
  '6.5rem', // Developer tools
  '1.75rem', // Data
  '5.75rem', // Communication
  '4.25rem', // Automation
  '3.75rem', // Lifestyle
  '3.5rem', // Research
];

/** One category filter. Inside a `SkeletonProvider` it draws the pill's own
 *  box around a shimmering label `w` wide. */
function CategoryPill({ label, active = false, onClick, w }: {
  label?: string;
  active?: boolean;
  onClick?: () => void;
  w?: string;
}) {
  if (useSkeleton()) {
    return <span class="pill-bar-btn" aria-hidden="true"><SkText w={w} /></span>;
  }
  return (
    <button type="button" class={`pill-bar-btn${active ? ' active' : ''}`} aria-pressed={active} onClick={onClick}>
      {label}
    </button>
  );
}

/** Loading placeholder that MIRRORS the loaded layout (category-pills bar above
 *  the list) so the list rows don't jump down when the catalog lands and the
 *  real `.app-store-filter-pills` bar appears. On touch the bar is one line, so
 *  only a category-less load settles up. On a fine pointer the bar wraps, so a
 *  catalog with fewer categories than the skeleton can settle up by a line.
 *  Pure/hookless so the unit test can inspect its vnode tree (`ListSkeletonOf`
 *  stays an uninvoked child). Only ever rendered inside `<LoadingFade>`, whose
 *  wrapper is already `aria-hidden`, so no aria treatment is needed here.
 *  Exported for the unit test that pins the no-jump regression. */
export function StoreTabSkeleton() {
  return (
    <div class="app-store">
      <SkeletonProvider>
        <div class="pill-bar app-store-filter-pills">
          {SKELETON_PILL_WIDTHS.map((w, i) => <CategoryPill w={w} key={i} />)}
        </div>
      </SkeletonProvider>
      <ListSkeletonOf fill containerClass="list-rows app-store-plugins" row={() => <PluginStoreRow />} />
    </div>
  );
}

function matchesQuery(plugin: MarketplacePlugin, query: string): boolean {
  if (!query) return true;
  return (
    plugin.name.toLowerCase().includes(query) ||
    plugin.description.toLowerCase().includes(query) ||
    plugin.marketplace_name.toLowerCase().includes(query)
  );
}

/** Marks an orphan row's synthetic `marketplace_id`. The engine builds a real id
 *  out of ASCII alphanumerics and dashes alone, so no registered marketplace can
 *  take this shape. `orphanRow` and the marketplace filter share the constant so
 *  the two cannot drift apart. */
const ORPHAN_MARKETPLACE_PREFIX = 'installed:';

/** An installed plugin whose marketplace is no longer registered won't appear in
 *  the marketplace catalog scan — synthesize a catalog row for it so it still
 *  lists (and stays uninstallable) under both All and Installed. It carries no
 *  marketplace metadata (no description / categories), so it renders with just
 *  its name, "Installed vX" badge, content chips, file count, and Uninstall. */
function orphanRow(p: InstalledPlugin): MarketplacePlugin {
  return {
    marketplace_id: `${ORPHAN_MARKETPLACE_PREFIX}${p.id}`,
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
    modified: p.modified,
    modified_paths: p.modified_paths,
    engine_requirement: p.engine_requirement,
    // Already installed, so there is nothing to install and nothing to block.
    engine_compatible: true,
    media: p.media,
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

/** One entry in the marketplace filter dropdown. */
export interface MarketplaceOption {
  id: string;
  name: string;
}

/** The catalog rows in scope right now, straight off the store. Both halves of
 *  the panel need them and neither can hand them to the other: the dropdown is
 *  in the filter bar and the list is below it. Calling this twice recomputes a
 *  small list, which is cheaper than a shared signal written during render. */
export function pluginRowsInScope(
  catalog: Loadable<{ plugins: MarketplacePlugin[] }>,
  installed: Loadable<InstalledPlugin[]>,
  installedOnly: boolean,
): MarketplacePlugin[] {
  return buildRows(
    catalog.status === 'loaded' ? catalog.data.plugins : [],
    installed.status === 'loaded' ? installed.data : [],
    installedOnly,
  );
}

/** The marketplaces the current rows actually come from, deduped and sorted by
 *  name. Reads the rows rather than the registry, so the dropdown never offers a
 *  marketplace with nothing under the Installed-only toggle.
 *
 *  This skips orphan rows: their marketplace is gone, so the id is synthetic and
 *  the name slot holds the plugin's source instead. An entry built from one would
 *  name a marketplace the user cannot browse. Pure + exported for the unit
 *  test. */
export function availableMarketplaces(rows: MarketplacePlugin[]): MarketplaceOption[] {
  const byId = new Map<string, string>();
  for (const row of rows) {
    if (row.marketplace_id.startsWith(ORPHAN_MARKETPLACE_PREFIX)) continue;
    if (!byId.has(row.marketplace_id)) byId.set(row.marketplace_id, row.marketplace_name);
  }
  return Array.from(byId, ([id, name]) => ({ id, name })).sort((a, b) => a.name.localeCompare(b.name));
}

/** The selected marketplace, or null for All. A selection the rows no longer
 *  offer (the toggle flipped, the marketplace was removed) resolves to All, so
 *  the list can never silently show nothing.
 *
 *  Resolving rather than resetting is what keeps this safe to call during a
 *  render. A setter there would fight the render it runs inside. Pure +
 *  exported for the unit test. */
export function resolveActiveMarketplace(
  selected: string | null,
  available: MarketplaceOption[],
): MarketplaceOption | null {
  return available.find((m) => m.id === selected) ?? null;
}

/** The catalog list's one filter expression: search query AND marketplace AND
 *  category, each inactive when it is empty or null. An active marketplace drops
 *  the orphan rows, which is right: their marketplace is no longer registered.
 *  Pure + exported for the unit test. */
export function matchesFilters(
  plugin: MarketplacePlugin,
  query: string,
  marketplaceId: string | null,
  category: string | null,
): boolean {
  return (
    matchesQuery(plugin, query) &&
    (!marketplaceId || plugin.marketplace_id === marketplaceId) &&
    (!category || plugin.categories.includes(category))
  );
}

/** Whether any filter could be hiding a deep-linked row, so a missing row does
 *  not yet prove the plugin is gone. Each of the three narrows the list, and
 *  giving up under one loses the notification's target for good. Pure +
 *  exported for the unit test. */
export function pluginDeepLinkCouldBeFiltered(
  query: string,
  marketplaceId: string | null,
  category: string | null,
): boolean {
  return !!query.trim() || !!marketplaceId || !!category;
}

/** Whether BOTH plugin data sources have SETTLED (loaded or failed) — the gate
 *  for releasing the list loading skeleton (see the call site for the why). A
 *  *failed* source counts as settled so a catalog-scan failure can't hang the
 *  skeleton. Pure + exported for the unit test. */
export function pluginRowsSettled(
  catalog: Loadable<unknown>,
  installed: Loadable<unknown>,
): boolean {
  const settled = (l: Loadable<unknown>) => l.status === 'loaded' || l.status === 'failed';
  return settled(catalog) && settled(installed);
}

/** What an unfiltered, empty catalog list says.
 *
 *  A registered marketplace is listed the moment its registry write lands, and
 *  its plugins are unknown until the scan has cloned the repo. So "none" is
 *  wrong for the seconds that takes, and it is wrong at the one moment the user
 *  is most likely to be looking: right after registering their first
 *  marketplace. Pure + exported for the unit test. */
export function emptyCatalogMessage(scanning: boolean): string {
  return scanning ? 'Scanning marketplaces…' : 'No plugins found.';
}

/** The panel's refresh. The catalog half asks for a fresh scan: the
 *  mount's re-read returns the cached scan. */
export function refreshPluginsPanel(): Promise<unknown> {
  return Promise.all([
    rescanPluginCatalogAndSettle(),
    loadInstalledPlugins(),
  ]);
}

export function StoreTab() {
  const installedOnly = pluginsInstalledOnly.value;
  const catLoadable = marketplaceCatalog.value;
  const instLoadable = installedPlugins.value;
  // `primary` is the source whose FAILED state we surface and whose `loaded` we
  // gate the final render on: Installed renders from the installed projection
  // (catalog best-effort, for the update status), All from the catalog (installed
  // list best-effort, for orphan coverage). Both are fetched on mount, so
  // flipping the toggle never re-flashes the spinner.
  const primary = installedOnly ? instLoadable : catLoadable;
  // The SKELETON, though, must wait for BOTH sources to settle — not just the
  // mode's primary. In Installed mode the primary is the fast installed
  // projection (a local disk scan), but the rows are enriched from the catalog
  // (descriptions, categories, the category-filter bar, the update_available
  // status), and the catalog scan clones marketplace repos — far slower. Gating
  // on the fast source alone released the skeleton early, so the list painted
  // bare orphan rows and then visibly reorganized when the catalog landed. A
  // failed source counts as settled (best-effort), so a catalog-scan failure
  // still falls back to orphan rows in Installed mode (and the `primary` failed
  // branch in All mode) instead of hanging the skeleton forever. On a same-session
  // revisit both are already `loaded` (setLoadingIfFresh keeps them so), so this
  // is true and the stale list shows immediately and refreshes in place.
  const settled = pluginRowsSettled(catLoadable, instLoadable);
  const showLoading = useDelayedFlag(!settled);
  const [installingSource, setInstallingSource] = useState<string | null>(null);
  // The category pills show a view of the list, not a setting, so the choice
  // never reaches preferences. Same for the marketplace dropdown above.
  const [selectedCategory, setSelectedCategory] = useState<string | null>(null);

  useEffect(() => {
    // Refresh both on mount (= each time the panel opens). The catalog gives
    // status + categories; the installed list gives orphan coverage and drives
    // the Installed view. refreshPluginCatalog only shows the spinner when the
    // catalog is still fresh, so a revisit re-fetch doesn't flash loading.
    //
    // The mount fetch is the whole refresh. `entityReferences.ts` reloads the
    // catalog on `PluginMarketplaceRegistered` and `PluginMarketplaceRemoved`,
    // which is the subscription this surface owes. So no interval, and no focus
    // or visibility listener standing in for one: `.claude/rules/frontend.md`
    // bans each. A scan git-clones every marketplace, seconds per repo.
    void refreshPluginCatalog();
    void loadInstalledPlugins();
  }, []);
  usePanelRefresh('plugins', refreshPluginsPanel);

  // Notification deep-link (navigate_ui target `plugins`): once the list has
  // rendered, scroll the targeted plugin's row into view and pulse it. The
  // target is consumed only on a successful scroll OR when the row is genuinely
  // absent with EVERY filter off, so a leftover one cannot swallow the
  // deep-link. Clearing it re-runs this (all three are deps) and the scroll then
  // lands.
  //
  // All three, not just the search box. The marketplace choice is a signal, so
  // it outlives the panel. It can already be narrowing the list on the mount the
  // deep-link arrives at.
  useEffect(() => {
    const target = pluginScrollTarget.value;
    // Gate on `settled`, not `primary.status` — the rows (and their
    // `data-plugin-id` anchors) only mount once StoreTabLoaded renders, which now
    // waits for both sources to settle.
    if (!target || !settled) return;
    const el = document.querySelector<HTMLElement>(`[data-plugin-id="${CSS.escape(target)}"]`);
    if (el) {
      el.scrollIntoView({ block: 'center', behavior: scrollBehavior() });
      // Shared navigation focus marker: a sticky background highlight that dissolves
      // on the user's next action, never before its hold has elapsed
      // (components/shared/focusMarker.ts). Same look as chat + settings.
      applyNavFocus(el);
      pluginScrollTarget.value = null;
    } else if (!pluginDeepLinkCouldBeFiltered(appSearchQuery.value, pluginsMarketplaceFilter.value, selectedCategory)) {
      // Row not in the list and nothing is filtering it out → the plugin is
      // genuinely gone (uninstalled / stale target). Give up so it can't linger.
      pluginScrollTarget.value = null;
    }
  }, [
    pluginScrollTarget.value,
    settled,
    appSearchQuery.value,
    installedOnly,
    pluginsMarketplaceFilter.value,
    selectedCategory,
  ]);

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
      <LoadingFade showSkeleton={showLoading} skeleton={<StoreTabSkeleton />}>
        {/* Render the loaded view only once BOTH sources have settled, so the
            skeleton shows alone (no bare orphan rows peeking through it) and the
            first painted content is already in final shape. After the early
            `failed` return above, `settled` implies the primary is loaded. */}
        {settled ? (
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

  // Through the shared helper, so the dropdown in the filter bar and this list
  // cannot drift on what "in scope" means.
  const rows = pluginRowsInScope(catLoadable, instLoadable, installedOnly);

  // Category filter — derived from the rows actually in scope (after the
  // install-state toggle) so a category with nothing under the current toggle
  // isn't offered. A stale selection (rows changed) falls back to "All" so the
  // list never silently shows nothing.
  const availableCategories = Array.from(new Set(rows.flatMap((p) => p.categories))).sort();
  const activeCategory =
    selectedCategory && availableCategories.includes(selectedCategory) ? selectedCategory : null;

  // The marketplace filter is the filter bar's dropdown, and its choice arrives
  // as a signal. Resolving it against these same rows is what keeps the two in
  // step: a selection the rows no longer offer reads as All in both places.
  const activeMarketplace = resolveActiveMarketplace(
    pluginsMarketplaceFilter.value,
    availableMarketplaces(rows),
  );

  const plugins = rows.filter((p) =>
    matchesFilters(p, query, activeMarketplace?.id ?? null, activeCategory),
  );

  return (
    <div class="app-store">
      {availableCategories.length > 0 && (
        <div class="pill-bar app-store-filter-pills" role="group" aria-label="Filter by category">
          <CategoryPill label="All" active={!activeCategory} onClick={() => setSelectedCategory(null)} />
          {availableCategories.map((c) => (
            <CategoryPill
              key={c}
              label={categoryLabel(c)}
              active={activeCategory === c}
              onClick={() => setSelectedCategory(activeCategory === c ? null : c)}
            />
          ))}
        </div>
      )}

      {plugins.length === 0 ? (
        <div class="empty-state">
          {query ? (
            <p>No plugins match "{appSearchQuery.value.trim()}".</p>
          ) : activeMarketplace ? (
            <p>No plugins from {activeMarketplace.name}.</p>
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
            <p>{emptyCatalogMessage(marketplaceScanning.value)}</p>
          )}
        </div>
      ) : (
        <div class="list-rows app-store-plugins">
          {plugins.map((plugin) => (
            <PluginStoreRow
              plugin={plugin}
              installingSource={installingSource}
              stageInstall={stageInstall}
              key={`${plugin.marketplace_id}-${plugin.id}`}
            />
          ))}
        </div>
      )}
    </div>
  );
}

interface PluginStoreRowProps {
  plugin: MarketplacePlugin;
  installingSource: string | null;
  stageInstall: (plugin: MarketplacePlugin) => void;
}

/** Self-skeletonizing plugin catalog row: rendered with no props inside a
 *  SkeletonProvider (`<PluginStoreRow />`) it draws itself as a loading
 *  placeholder via the Sk* leaves; with real props it renders the catalog row
 *  normally. Props are optional only to support the skeleton call; real call
 *  sites pass them all. A tap on the row opens the plugin detail page. */
export function PluginStoreRow({ plugin, installingSource, stageInstall }: Partial<PluginStoreRowProps>) {
  const sk = useSkeleton();
  return (
    <div
      class={`list-row app-store-plugin-row${sk ? '' : ' clickable'}`}
      data-plugin-id={sk ? undefined : plugin?.id}
      onClick={plugin ? () => openPluginDetail(plugin) : undefined}
    >
      <div class="app-store-plugin-icon">
        <PluginIcon plugin={plugin} />
      </div>
      <div class="list-row-info">
        <PluginTitle plugin={plugin} nameClass="title list-row-name" />
        {(sk || plugin?.description) && (
          <SkText class="app-store-plugin-description" as="div" w="18rem">{plugin?.description}</SkText>
        )}
        <PluginMeta plugin={plugin} />
      </div>
      <PluginActions
        plugin={plugin}
        busy={!!plugin && installingSource === plugin.source}
        onInstall={stageInstall}
      />
    </div>
  );
}
