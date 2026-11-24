import { useEffect, useRef, useState } from 'preact/hooks';
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
import { categoryLabel, PluginIcon, PluginRowAction, PluginRowNote } from './pluginCard';
import { openSettingsSubview } from '../../store/actions/menu';
import { AddOfficialMarketplaceButton } from './AddOfficialMarketplaceButton';
import { applyNavFocus } from '../shared/focusMarker';
import { scrollBehavior } from '../../utils/motion';
import { PluginDescription } from './PluginDescription';
import { SectionHeader } from '../shared/SectionHeader';
import { Disclosure } from '../shared/Disclosure';

export function openMarketplaceSettings() {
  openSettingsSubview('marketplaces');
}

const SKELETON_PILL_WIDTHS = [
  '1rem',
  '5.25rem',
  '3rem',
  '2.5rem',
  '6.5rem',
  '1.75rem',
  '5.75rem',
  '4.25rem',
  '3.75rem',
  '3.5rem',
];

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

const ORPHAN_MARKETPLACE_PREFIX = 'installed:';

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
    engine_compatible: true,
    media: p.media,
  };
}

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

export interface MarketplaceOption {
  id: string;
  name: string;
}

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

export function availableMarketplaces(rows: MarketplacePlugin[]): MarketplaceOption[] {
  const byId = new Map<string, string>();
  for (const row of rows) {
    if (row.marketplace_id.startsWith(ORPHAN_MARKETPLACE_PREFIX)) continue;
    if (!byId.has(row.marketplace_id)) byId.set(row.marketplace_id, row.marketplace_name);
  }
  return Array.from(byId, ([id, name]) => ({ id, name })).sort((a, b) => a.name.localeCompare(b.name));
}

export function resolveActiveMarketplace(
  selected: string | null,
  available: MarketplaceOption[],
): MarketplaceOption | null {
  return available.find((m) => m.id === selected) ?? null;
}

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

export function pluginDeepLinkCouldBeFiltered(
  query: string,
  marketplaceId: string | null,
  category: string | null,
): boolean {
  return !!query.trim() || !!marketplaceId || !!category;
}

export function pluginRowsSettled(
  catalog: Loadable<unknown>,
  installed: Loadable<unknown>,
): boolean {
  const settled = (l: Loadable<unknown>) => l.status === 'loaded' || l.status === 'failed';
  return settled(catalog) && settled(installed);
}

export function emptyCatalogMessage(scanning: boolean): string {
  return scanning ? 'Scanning marketplaces…' : 'No plugins found.';
}

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
  const primary = installedOnly ? instLoadable : catLoadable;
  const settled = pluginRowsSettled(catLoadable, instLoadable);
  const showLoading = useDelayedFlag(!settled);
  const [installingSource, setInstallingSource] = useState<string | null>(null);
  const [selectedCategory, setSelectedCategory] = useState<string | null>(null);
  const [collapsedSections, setCollapsedSections] = useState<ReadonlySet<PluginSectionId>>(new Set());
  const unfoldedForTarget = useRef<string | null>(null);

  function toggleSection(id: PluginSectionId) {
    setCollapsedSections((prev) => {
      const next = new Set(prev);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  }

  useEffect(() => {
    void refreshPluginCatalog();
    void loadInstalledPlugins();
  }, []);
  usePanelRefresh('plugins', refreshPluginsPanel);

  useEffect(() => {
    const target = pluginScrollTarget.value;
    if (!target || !settled) return;
    if (collapsedSections.size > 0 && unfoldedForTarget.current !== target) {
      unfoldedForTarget.current = target;
      setCollapsedSections(new Set());
      return;
    }
    const el = document.querySelector<HTMLElement>(`[data-plugin-id="${CSS.escape(target)}"]`);
    if (el) {
      el.scrollIntoView({ block: 'center', behavior: scrollBehavior() });
      applyNavFocus(el);
      pluginScrollTarget.value = null;
    } else if (!pluginDeepLinkCouldBeFiltered(appSearchQuery.value, pluginsMarketplaceFilter.value, selectedCategory)) {
      pluginScrollTarget.value = null;
    }
  }, [
    pluginScrollTarget.value,
    settled,
    appSearchQuery.value,
    installedOnly,
    pluginsMarketplaceFilter.value,
    selectedCategory,
    collapsedSections,
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
        {settled ? (
          <StoreTabLoaded
            installedOnly={installedOnly}
            installingSource={installingSource}
            selectedCategory={selectedCategory}
            setSelectedCategory={setSelectedCategory}
            collapsedSections={collapsedSections}
            toggleSection={toggleSection}
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
  collapsedSections,
  toggleSection,
  stageInstall,
}: {
  installedOnly: boolean;
  installingSource: string | null;
  selectedCategory: string | null;
  setSelectedCategory: (c: string | null) => void;
  collapsedSections: ReadonlySet<PluginSectionId>;
  toggleSection: (id: PluginSectionId) => void;
  stageInstall: (plugin: MarketplacePlugin) => void;
}) {
  const catLoadable = marketplaceCatalog.value;
  const instLoadable = installedPlugins.value;

  const catalog = catLoadable.status === 'loaded' ? catLoadable.data : null;
  const installed = instLoadable.status === 'loaded' ? instLoadable.data : [];
  const hasMarketplaces = (catalog?.marketplaces.length ?? 0) > 0;
  const query = appSearchQuery.value.trim().toLowerCase();

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

  const rows = pluginRowsInScope(catLoadable, instLoadable, installedOnly);

  const availableCategories = Array.from(new Set(rows.flatMap((p) => p.categories))).sort();
  const activeCategory =
    selectedCategory && availableCategories.includes(selectedCategory) ? selectedCategory : null;

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
        <PluginRowSections
          plugins={plugins}
          installedOnly={installedOnly}
          collapsed={collapsedSections}
          onToggle={toggleSection}
          instant={!!pluginScrollTarget.value}
          installingSource={installingSource}
          stageInstall={stageInstall}
        />
      )}
    </div>
  );
}

export type PluginSectionId = 'updates' | 'rest';

export function splitOutdated(plugins: MarketplacePlugin[]): {
  updates: MarketplacePlugin[];
  rest: MarketplacePlugin[];
} {
  const updates: MarketplacePlugin[] = [];
  const rest: MarketplacePlugin[] = [];
  for (const p of plugins) (p.status === 'update_available' ? updates : rest).push(p);
  return { updates, rest };
}

export function PluginRowSections({
  plugins,
  installedOnly,
  collapsed,
  onToggle,
  instant = false,
  installingSource,
  stageInstall,
}: {
  plugins: MarketplacePlugin[];
  installedOnly: boolean;
  collapsed: ReadonlySet<PluginSectionId>;
  onToggle: (id: PluginSectionId) => void;
  instant?: boolean;
  installingSource: string | null;
  stageInstall: (plugin: MarketplacePlugin) => void;
}) {
  const { updates, rest } = splitOutdated(plugins);
  const sectioned = updates.length > 0;
  const rows = (list: MarketplacePlugin[]) =>
    list.map((plugin) => (
      <PluginStoreRow
        plugin={plugin}
        installingSource={installingSource}
        stageInstall={stageInstall}
        key={`${plugin.marketplace_id}-${plugin.id}`}
      />
    ));
  return (
    <div class="list-rows app-store-plugins">
      <Disclosure open={sectioned} instant={instant}>
        <SectionHeader
          title="Updates available"
          count={updates.length}
          collapsed={collapsed.has('updates')}
          onToggle={() => onToggle('updates')}
        />
        <Disclosure open={!collapsed.has('updates')} instant={instant} bodyClass="list-rows">
          {rows(updates)}
        </Disclosure>
      </Disclosure>
      <Disclosure open={sectioned && rest.length > 0} instant={instant}>
        <SectionHeader
          title={installedOnly ? 'Up to date' : 'All plugins'}
          count={rest.length}
          collapsed={collapsed.has('rest')}
          onToggle={() => onToggle('rest')}
        />
      </Disclosure>
      <Disclosure open={!sectioned || !collapsed.has('rest')} instant={instant} bodyClass="list-rows">
        {rows(rest)}
      </Disclosure>
    </div>
  );
}

interface PluginStoreRowProps {
  plugin: MarketplacePlugin;
  installingSource: string | null;
  stageInstall: (plugin: MarketplacePlugin) => void;
}

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
        <div class="app-store-plugin-title-row">
          <SkText class="title list-row-name" w="9rem">{plugin?.name}</SkText>
        </div>
        {(sk || plugin?.description) && (
          <PluginDescription class="app-store-plugin-description" w="18rem" description={plugin?.description} />
        )}
        <PluginRowNote plugin={plugin} />
      </div>
      <PluginRowAction
        plugin={plugin}
        busy={!!plugin && installingSource === plugin.source}
        onInstall={stageInstall}
      />
    </div>
  );
}
