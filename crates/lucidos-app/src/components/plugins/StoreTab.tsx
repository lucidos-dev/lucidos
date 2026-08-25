import { useEffect, useState } from 'preact/hooks';
import {
  marketplaceCatalog,
  installedPlugins,
  appSearchQuery,
  pluginsInstalledOnly,
  setPluginsInstalledOnly,
  pluginScrollTarget,
} from '../../store/store';
import type { InstalledPlugin, Loadable, MarketplacePlugin } from '../../store/types';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf, useSkeleton, SkText, SkBlock } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { refreshPluginCatalog } from '../../store/actions/plugin-marketplaces';
import { installMarketplacePlugin } from '../../store/actions/plugin-install';
import { loadInstalledPlugins } from '../../store/actions/plugins';
import { openAppById } from '../../store/actions/apps';
import { focusThread } from '../../store/actions/threads';
import { uninstallMarketplacePlugin } from '../../store/actions/plugin-uninstall';
import { ProposeUpstreamButton } from './ProposeUpstreamButton';
import { openSettingsSubview } from '../../store/actions/menu';
import { AddOfficialMarketplaceButton } from './AddOfficialMarketplaceButton';
import { contentLabel } from './pluginContent';
import { applyNavFocus } from '../shared/focusMarker';

/** Jump to Settings → Marketplaces from anywhere. One call: `openSettingsSubview`
 *  lands the Settings panel and the sub-section together, so the jump is a single
 *  nav-history entry (the same shape the navigate_ui settings deep link uses). */
function openMarketplaceSettings() {
  openSettingsSubview('marketplaces');
}

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

/** Widths of the placeholder pills in the loading skeleton — sized to mirror a
 *  FULLY-populated category bar (`All` + the ~9 controlled-vocabulary categories,
 *  see `core/plugins.rs` PLUGIN_CATEGORIES), varied per the real labels'
 *  approximate widths. The count matters as much as the widths: a populated
 *  catalog's real bar wraps to ~2 lines in a typical pane (more at a larger UI
 *  scale), so the skeleton must reserve ~2 lines too — six narrow pills fit on
 *  one line and let the list jump down a line when the real bar lands. */
const SKELETON_PILL_WIDTHS = [
  '2.25rem', // All
  '6.5rem', // Productivity
  '4.25rem', // Finance
  '3.75rem', // Health
  '7.75rem', // Developer tools
  '3rem', // Data
  '7rem', // Communication
  '5.5rem', // Automation
  '5rem', // Lifestyle
  '4.75rem', // Research
];

/** Loading placeholder that MIRRORS the loaded layout (category-pills bar above
 *  the list) so the list rows don't jump down when the catalog lands and the
 *  real `.app-store-category-filter` appears. The pills bar is sized to a fully
 *  populated catalog (~2 lines, see SKELETON_PILL_WIDTHS) because that is the
 *  height the real bar settles at — reserving only one line let the rows shift
 *  down a line on a cold reload once the real (wrapping) bar rendered. A sparse
 *  catalog now settles UP by at most a line, far less jarring than the old
 *  always-downward shift; a rare category-less load has no real bar at all and
 *  still settles up by the whole bar (accepted — the common case is populated).
 *  Pure/hookless so the unit test can inspect its vnode tree (`ListSkeletonOf`
 *  stays an uninvoked child). Only ever rendered inside `<LoadingFade>`, whose
 *  wrapper is already `aria-hidden`, so no aria treatment is needed here.
 *  Exported for the unit test that pins the no-jump regression. */
export function StoreTabSkeleton() {
  return (
    <div class="app-store">
      <div class="app-store-category-filter">
        {SKELETON_PILL_WIDTHS.map((w, i) => (
          <div class="app-store-category-pill-skeleton" style={{ width: w }} key={i} />
        ))}
      </div>
      <ListSkeletonOf fill containerClass="list-rows app-store-plugins" row={() => <PluginStoreRow />} />
    </div>
  );
}

/** Tooltip for the "Modified" badge, listing the changed paths (capped so the
 *  tooltip stays readable).
 *
 *  It used to warn that a future update would overwrite these changes. An
 *  update now merges them, so saying so would be exactly backwards. */
function modifiedTooltip(paths?: string[]): string {
  const base = 'You have locally changed this plugin. An update merges your changes into the new version where it can.';
  if (!paths || paths.length === 0) return base;
  const shown = paths.slice(0, 6).join(', ');
  const more = paths.length > 6 ? `, +${paths.length - 6} more` : '';
  return `${base} Changed: ${shown}${more}`;
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
    modified: p.modified,
    modified_paths: p.modified_paths,
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
    // Gate on `settled`, not `primary.status` — the rows (and their
    // `data-plugin-id` anchors) only mount once StoreTabLoaded renders, which now
    // waits for both sources to settle.
    if (!target || !settled) return;
    const el = document.querySelector<HTMLElement>(`[data-plugin-id="${CSS.escape(target)}"]`);
    if (el) {
      el.scrollIntoView({ block: 'center', behavior: 'smooth' });
      // Shared navigation focus marker: a sticky background highlight that dissolves
      // on the user's next action, never before its hold has elapsed
      // (components/shared/focusMarker.ts). Same look as chat + settings.
      applyNavFocus(el);
      pluginScrollTarget.value = null;
    } else if (!appSearchQuery.value.trim()) {
      // Row not in the list and nothing is filtering it out → the plugin is
      // genuinely gone (uninstalled / stale target). Give up so it can't linger.
      pluginScrollTarget.value = null;
    }
  }, [pluginScrollTarget.value, settled, appSearchQuery.value, installedOnly]);

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
 *  sites pass them all. */
function PluginStoreRow({ plugin, installingSource, stageInstall }: Partial<PluginStoreRowProps>) {
  const sk = useSkeleton();
  // 'available' is the only not-yet-installed state; 'installed' and
  // 'update_available' both mean the plugin is on disk → uninstallable.
  const isInstalled = !!plugin && plugin.status !== 'available';
  const busy = !!plugin && installingSource === plugin.source;
  const action = plugin ? cardPrimaryAction(plugin) : { kind: 'none' as const };
  let label: string;
  let onPrimary: (() => void) | undefined;
  let primaryDisabled = busy;
  switch (action.kind) {
    case 'install':
      label = action.label;
      onPrimary = () => plugin && void stageInstall?.(plugin);
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
      data-plugin-id={sk ? undefined : plugin?.id}
    >
      <div class="list-row-info">
        <div class="app-store-plugin-title-row">
          <SkText class="title list-row-name" w="9rem">{plugin?.name}</SkText>
          {(sk || plugin) && (
            <SkBlock w="5rem" h="1rem" round>
              <span class={`app-store-status app-store-status-${plugin?.status}`}>
                {plugin && statusLabel(plugin)}
              </span>
            </SkBlock>
          )}
          {plugin?.modified && (
            <span class="app-store-modified-chip" data-tooltip={modifiedTooltip(plugin.modified_paths)}>
              Modified
            </span>
          )}
        </div>
        {(sk || plugin?.description) && (
          <SkText class="list-row-details" as="div" w="18rem">{plugin?.description}</SkText>
        )}
        <div class="app-store-plugin-meta">
          {(sk || plugin?.marketplace_name) && (
            <SkText w="6rem">{plugin?.marketplace_name}</SkText>
          )}
          <SkText w="3rem">{plugin && fileCountLabel(plugin.files_count)}</SkText>
          {sk && <SkBlock w="4rem" h="1rem" round />}
          {plugin?.content.map((kind) => (
            <span class="app-store-content-chip" key={kind}>
              {contentLabel(kind)}
            </span>
          ))}
          {plugin?.categories.map((c) => (
            <span class="app-store-category-chip" key={`cat-${c}`}>
              {categoryLabel(c)}
            </span>
          ))}
        </div>
      </div>
      <div class="list-row-actions">
        {plugin?.modified && (
          <ProposeUpstreamButton pluginId={plugin.id} pluginName={plugin.name} />
        )}
        {isInstalled && (
          <button
            class="action-btn action-btn-danger"
            type="button"
            onClick={() => plugin && void uninstallMarketplacePlugin(plugin)}
          >
            Uninstall
          </button>
        )}
        <SkBlock w="4.5rem" h="2rem" round>
          <button
            class="action-btn"
            type="button"
            disabled={primaryDisabled}
            onClick={onPrimary}
          >
            {busy ? 'Staging' : label}
          </button>
        </SkBlock>
      </div>
    </div>
  );
}
