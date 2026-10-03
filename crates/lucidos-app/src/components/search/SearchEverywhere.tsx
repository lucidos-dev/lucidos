import { useState, useRef, useEffect, useCallback, useMemo } from 'preact/hooks';
import { searchEverywhereOpen, searchEverywhereAnchor, appsList, artifacts, triggers, settingsScrollTarget, focusedPane } from '../../store/store';
import { Overlay } from '../shared/Overlay';
import { searchEverywhere, type SearchCategory, type SearchResultItem, type ServerSearchCategory } from '../../api/client';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { openFilePreview } from '../../store/actions/artifacts';
import { openAppById } from '../../store/actions/apps';
import { openSettingsSubview, switchMenuItem } from '../../store/actions/menu';
import { viewChangeDiffById } from '../../store/actions/repositories';
import { navigateToTrigger } from '../../store/actions/triggers';
import { focusPaneMainControl } from '../layout/paneFocus';
import { searchResultDestinationPane, searchResultIconCategory } from './searchEverywhereActions';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { paneOfFocus, paneUnder, usePaneCentre } from '../../hooks/usePaneCentre';
import { toFailed, type Loadable } from '../../store/types';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText } from '../shared/Skeleton';
import { RECENTS_KEY } from '../../store/actions/entityReferences';
import { CloseIcon, ClearIcon } from '../shared/icons';
import { SearchField } from '../shared/SearchField';
import { CategoryIcon } from '../shared/CategoryIcon';
import { getSettingsSearchResults, findSettingsEntry } from './searchIndex';
import { getMenuSearchResults, findMenuSearchEntry } from './menuIndex';
import { OVERVIEW_LIMIT, rankedSections, tabAccessibleName, tabHits, type TabHits } from './searchSections';
import './SearchEverywhere.css';

const CATEGORIES: { id: SearchCategory; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'apps', label: 'Apps' },
  { id: 'files', label: 'Files' },
  { id: 'settings', label: 'Settings' },
  { id: 'threads', label: 'Threads' },
  { id: 'triggers', label: 'Triggers' },
  { id: 'changes', label: 'Changes' },
  // Last, in the same place SECTION_ORDER breaks ties for the All tab. The
  // strip never reorders: only the All tab's sections follow the hits.
  { id: 'menu', label: 'Menu' },
];

type LocalCategory = 'settings' | 'menu';
type ServerSection = Exclude<ServerSearchCategory, 'all'>;

/** Asked one request each, so a fast category never waits on a slow one:
 *  thread search embeds the query, the rest are listings. */
const SERVER_SECTIONS: ServerSection[] = ['apps', 'files', 'threads', 'triggers', 'changes'];

/** Keystrokes coalesce this long before the engine is asked. Local hits skip it. */
const SERVER_DEBOUNCE_MS = 150;

/** The two categories the frontend answers itself, never the engine. */
function isLocalCategory(category: SearchCategory): category is LocalCategory {
  return category === 'settings' || category === 'menu';
}

function localSection(section: LocalCategory, query: string, limit: number): SearchResultItem[] {
  return section === 'settings'
    ? getSettingsSearchResults(query, limit)
    : getMenuSearchResults(query, limit);
}

/** The local half of the overview, so it renders on the keystroke. */
function localOverview(query: string): Record<LocalCategory, SearchResultItem[]> {
  return { settings: localSection('settings', query, OVERVIEW_LIMIT), menu: localSection('menu', query, OVERVIEW_LIMIT) };
}

function sectionLabel(section: string): string {
  return CATEGORIES.find(c => c.id === section)?.label ?? section;
}

function itemKey(item: SearchResultItem): string {
  return `${item.category}:${item.id}`;
}

const MAX_RECENTS = 15;

function loadRecents(): SearchResultItem[] {
  try {
    const saved = localStorage.getItem(RECENTS_KEY);
    return saved ? JSON.parse(saved) : [];
  } catch { return []; }
}

function saveRecent(item: SearchResultItem) {
  const recents = loadRecents().filter(r => !(r.id === item.id && r.category === item.category));
  recents.unshift(item);
  if (recents.length > MAX_RECENTS) recents.length = MAX_RECENTS;
  localStorage.setItem(RECENTS_KEY, JSON.stringify(recents));
}

function validateRecents(recents: SearchResultItem[]): SearchResultItem[] {
  const validated = recents.filter(item => {
    switch (item.category) {
      case 'apps': {
        const apps = appsList.value;
        if (apps.status !== 'loaded') return true;
        return apps.data.some(a => a.id === item.id);
      }
      case 'files': {
        const files = artifacts.value;
        if (files.status !== 'loaded') return true;
        return files.data.includes(item.id);
      }
      case 'triggers': {
        const trigs = triggers.value;
        if (trigs.status !== 'loaded') return true;
        return trigs.data.some(t => t.id === item.id);
      }
      case 'threads':
        // This filter keeps every thread recent: `threadMap` holds only the
        // loaded window, and nothing on a cold start, so a miss proves nothing.
        // `dropDeletedThreads` and a not-found tap prune it instead.
        return true;
      case 'settings':
        // Recents are persisted verbatim, so an id retired by a Settings
        // restructure outlives the build that had it. `handleSelect` does
        // `if (!entry) break`, which closes the palette and navigates nowhere:
        // the same silent dead end the persisted nav stack got
        // `migrateSettingsSubview` for. Drop the row instead of listing it.
        return findSettingsEntry(item.id) !== undefined;
      case 'menu':
        // Same dead end as a retired settings id: a menu item dropped from
        // MENU_ITEMS outlives the build that had it, and `handleSelect` would
        // close the palette and navigate nowhere.
        return findMenuSearchEntry(item.id) !== undefined;
      case 'changes':
        return true;
    }
    return true;
  });
  if (validated.length < recents.length) {
    localStorage.setItem(RECENTS_KEY, JSON.stringify(validated));
  }
  return validated;
}

/** One hit. With no `item`, inside a `SkeletonProvider`, it is the results
 *  list's loading placeholder, and carries no `data-role`: the keyboard
 *  selection counts rows by it. */
function ResultRow({ item, index = 0, selected = false, onSelect, onHover }: {
  item?: SearchResultItem;
  index?: number;
  selected?: boolean;
  onSelect?: (item: SearchResultItem) => void;
  onHover?: (index: number) => void;
}) {
  return (
    <button
      data-role={item ? 'search-result' : undefined}
      class={`search-everywhere-result${selected ? ' selected' : ''}`}
      onMouseEnter={() => onHover?.(index)}
      onClick={() => item && onSelect?.(item)}
      tabIndex={item ? undefined : -1}
    >
      <SkBlock w="1rem" h="1rem" round>
        <span class="search-everywhere-result-icon">
          {item && <CategoryIcon category={searchResultIconCategory(item)} />}
        </span>
      </SkBlock>
      <span class="search-everywhere-result-info">
        <SkText class="search-everywhere-result-title" w="9rem">{item?.title}</SkText>
        {(!item || item.subtitle) && (
          <SkText class="search-everywhere-result-subtitle" w="14rem">{item?.subtitle}</SkText>
        )}
      </span>
    </button>
  );
}

/** The trailing row while engine categories are still out. Local hits and the
 *  categories that already answered stay above it. */
function PendingRow({ sections }: { sections: ServerSection[] }) {
  const names = sections.map(s => sectionLabel(s).toLowerCase()).join(', ');
  return (
    <div class="search-everywhere-pending" role="status">
      <span class="mini-spinner" aria-hidden="true" />
      <span>Searching {names}</span>
    </div>
  );
}

/** Engine answers, each stamped with the search it answers. A section whose
 *  stamp is not the current search is still out, whatever it last held. */
type ServerHits = { search: string; sections: Partial<Record<ServerSection, Loadable<SearchResultItem[]>>> };

const NO_SERVER_HITS: ServerHits = { search: '', sections: {} };
const NO_SECTIONS: ServerSection[] = [];

/** Asks the engine one request per section, once the keystrokes pause, and
 *  stamps each answer with `search`. Drops every answer on close, so a
 *  reopened palette asks again rather than showing old ones. */
function useServerHits(
  isOpen: boolean,
  search: string,
  query: string,
  sections: ServerSection[],
  limit: number | undefined,
): (section: ServerSection) => Loadable<SearchResultItem[]> {
  const [hits, setHits] = useState<ServerHits>(NO_SERVER_HITS);

  useEffect(() => {
    if (!isOpen) setHits(NO_SERVER_HITS);
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen || sections.length === 0) return;
    const controller = new AbortController();
    const settle = (section: ServerSection, state: Loadable<SearchResultItem[]>) => {
      if (controller.signal.aborted) return;
      setHits(prev => ({
        search,
        sections: { ...(prev.search === search ? prev.sections : {}), [section]: state },
      }));
    };
    const timer = setTimeout(() => {
      for (const section of sections) {
        searchEverywhere(query, section, { signal: controller.signal, limit }).then(
          data => settle(section, { status: 'loaded', data: data.results[section] ?? [] }),
          // Failed must not read as empty: the list says so under the rows.
          err => settle(section, toFailed(err)),
        );
      }
    }, SERVER_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [search, query, sections, limit, isOpen]);

  return section => (hits.search === search && hits.sections[section]) || { status: 'loading' };
}

export function SearchEverywhere() {
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState<SearchCategory>('all');
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [recents, setRecents] = useState<SearchResultItem[]>([]);
  const inputRef = useRef<HTMLInputElement>(null);
  const resultsRef = useRef<HTMLDivElement>(null);

  const isRecentsMode = !query && category === 'all';
  const isOpen = searchEverywhereOpen.value;
  // The overview answers the All tab and every tab's count, so it is keyed by
  // the query alone: switching tabs never asks it again.
  const overviewState = useServerHits(isOpen, query, query, query ? SERVER_SECTIONS : NO_SECTIONS, OVERVIEW_LIMIT);
  // A category tab also asks for its own full page.
  const pageSections = useMemo(
    () => (category === 'all' || isLocalCategory(category) ? NO_SECTIONS : [category]),
    [category],
  );
  const pageState = useServerHits(isOpen, `${category}\n${query}`, query, pageSections, undefined);

  const requested = category === 'all' ? (query ? SERVER_SECTIONS : NO_SECTIONS) : pageSections;
  const listState = category === 'all' ? overviewState : pageState;
  const pending = requested.filter(s => listState(s).status === 'loading');
  const failed = requested.flatMap(s => {
    const state = listState(s);
    return state.status === 'failed' ? [{ section: s, error: state.error }] : [];
  });
  // Placeholder rows only once a search has run past the delay gate. Hits
  // still show the instant they arrive; the fade only lets the rows go.
  const showSearchLoading = useDelayedFlag(pending.length > 0);

  // Over the pane whose header holds the button that opened it. Opened with
  // no button (a shortcut, the Lucidos menu), it follows the focused pane group.
  // Resolved once per open: every keystroke re-renders, and it measures layout.
  const anchor = searchEverywhereAnchor.value;
  const focus = focusedPane.value;
  const pane = useMemo(
    () => (isOpen ? paneUnder(anchor) ?? paneOfFocus(focus) : undefined),
    [isOpen, anchor, focus],
  );
  const paneCentre = usePaneCentre(pane);

  const close = useCallback(() => {
    searchEverywhereOpen.value = false;
    setQuery('');
    setCategory('all');
    setSelectedKey(null);
  }, []);

  useEffect(() => {
    if (isOpen) setRecents(validateRecents(loadRecents()));
  }, [isOpen]);

  // Auto-focus input on open
  useEffect(() => {
    if (isOpen && inputRef.current) {
      inputRef.current.focus();
    }
  }, [isOpen]);

  function handleSelect(item: SearchResultItem) {
    saveRecent(item);
    close();
    // After navigating, move real DOM focus into the destination pane's scroll
    // surface (the transcript for a thread; the body scroller / iframe for an app,
    // file, trigger, settings, or change) so a keyboard-driven selection continues
    // there — Arrow/Page keys scroll it — instead of stranding focus on <body> when
    // the modal closes. Typing still lands in the prompt via type-to-focus.
    // Desktop-only and a no-op when nothing is focusable — both the "(if any)"
    // cases — are handled inside focusPaneMainControl. Async navigations
    // (app/trigger/change resolve their target before revealing the content pane)
    // chain the focus off their promise so it lands after the destination renders.
    const focusDest = () => focusPaneMainControl(searchResultDestinationPane(item.category));
    switch (item.category) {
      case 'threads': focusThreadOrBootstrap(item.id); focusDest(); break;
      case 'files': openFilePreview(item.id); focusDest(); break;
      case 'apps': void openAppById(item.id).then(focusDest); break;
      case 'triggers': void navigateToTrigger(item.id).then(focusDest); break;
      case 'settings': {
        const entry = findSettingsEntry(item.id);
        if (!entry) break;
        // One call, one nav-history entry: openSettingsSubview lands the
        // Settings panel and the sub-section together.
        openSettingsSubview(entry.subview);
        // An anchored entry scrolls to a specific row; SettingsView's scroll
        // effect lands focus on that row's control (e.g. the Language dropdown).
        // A top-level subview entry has no anchor — focus its first control via
        // the pane instead. Calling focusDest for an anchored entry would grab
        // the panel's first control and fight the row focus.
        if (entry.anchor) settingsScrollTarget.value = entry.anchor;
        else focusDest();
        break;
      }
      case 'changes': void viewChangeDiffById(item.id).then(focusDest); break;
      case 'menu': {
        const entry = findMenuSearchEntry(item.id);
        if (!entry) break;
        // The id IS the destination, and switchMenuItem owns the data load, the
        // nav push and the pane reveal. Resolving the entry first keeps a
        // recents row from a retired build out of `activeMenuItem`.
        switchMenuItem(entry.id);
        focusDest();
        break;
      }
    }
  }

  const overviewLocal = useMemo(() => (query ? localOverview(query) : null), [query]);
  const pageLocal = useMemo(
    () => (isLocalCategory(category) ? localSection(category, query, 50) : []),
    [category, query],
  );

  const results: Record<string, SearchResultItem[]> = category === 'all' ? { ...overviewLocal } : {};
  if (isLocalCategory(category)) results[category] = pageLocal;
  for (const section of requested) {
    const state = listState(section);
    if (state.status === 'loaded') results[section] = state.data;
  }

  const sections = category === 'all' && !isRecentsMode ? rankedSections(results, query) : [];
  const flat = isRecentsMode
    ? recents
    : category === 'all' ? sections.flatMap(s => s.items) : results[category] ?? [];

  function hitsOnTab(tab: SearchCategory): TabHits {
    if (tab === 'all' || !overviewLocal) return { kind: 'unknown' };
    return isLocalCategory(tab)
      ? tabHits({ status: 'loaded', data: overviewLocal[tab] })
      : tabHits(overviewState(tab));
  }
  // The selection follows its row, so a category landing above it cannot move
  // the cursor onto another hit.
  const selectedIndex = selectedKey === null ? -1 : flat.findIndex(item => itemKey(item) === selectedKey);
  const selectAt = (index: number) => setSelectedKey(index >= 0 && flat[index] ? itemKey(flat[index]) : null);

  // Scroll selected result into view
  useEffect(() => {
    if (selectedIndex >= 0 && resultsRef.current) {
      const buttons = resultsRef.current.querySelectorAll('[data-role="search-result"]');
      const el = buttons[selectedIndex] as HTMLElement | undefined;
      el?.scrollIntoView({ block: 'nearest' });
    }
  }, [selectedIndex]);

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
      return;
    }

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      selectAt(Math.min(selectedIndex + 1, flat.length - 1));
      return;
    }

    if (e.key === 'ArrowUp') {
      e.preventDefault();
      selectAt(Math.max(selectedIndex - 1, -1));
      return;
    }

    if (e.key === 'Enter' && flat.length > 0) {
      e.preventDefault();
      handleSelect(flat[Math.max(selectedIndex, 0)]);
      return;
    }

    if (e.key === 'Tab') {
      e.preventDefault();
      const dir = e.shiftKey ? -1 : 1;
      const currentIdx = CATEGORIES.findIndex(c => c.id === category);
      const nextIdx = (currentIdx + dir + CATEGORIES.length) % CATEGORIES.length;
      setCategory(CATEGORIES[nextIdx].id);
    }
  }

  const hasResults = flat.length > 0;

  // keepMounted: iOS Safari PWA never unmounts the overlay — it toggles
  // visibility via CSS to avoid ghost pixels from the will-change compositing
  // layer. anchor: the search toggle, exempt from outside-dismiss so re-tapping
  // it closes (never reopens). Both contracts live in <Overlay>.
  function rows() {
    if (category === 'all' && !isRecentsMode) {
      return sections.map(({ section, items, offset }) => (
        <div key={section}>
          <div class="search-everywhere-section-header">{section}</div>
          {items.map((item, i) => (
            <ResultRow
              key={itemKey(item)}
              item={item}
              index={offset + i}
              selected={offset + i === selectedIndex}
              onSelect={handleSelect}
              onHover={selectAt}
            />
          ))}
        </div>
      ));
    }
    return flat.map((item, index) => (
      <ResultRow
        key={itemKey(item)}
        item={item}
        index={index}
        selected={index === selectedIndex}
        onSelect={handleSelect}
        onHover={selectAt}
      />
    ));
  }

  function searchBody() {
    if (!hasResults && pending.length > 0) return null;
    if (!hasResults && failed.length > 0 && failed.length === requested.length) {
      return <div class="search-everywhere-empty error-text">Search failed: {failed[0].error}</div>;
    }
    return (
      <>
        {hasResults ? rows() : (
          <div class="search-everywhere-empty">
            {query ? `No results for "${query}"` : `No ${sectionLabel(category).toLowerCase()}`}
          </div>
        )}
        {failed.map(({ section, error }) => (
          <div key={section} class="search-everywhere-note error-text">
            {sectionLabel(section)} search failed: {error}
          </div>
        ))}
        {showSearchLoading && pending.length > 0 && <PendingRow sections={pending} />}
      </>
    );
  }

  return (
    <Overlay
      open={isOpen}
      onClose={close}
      anchor={searchEverywhereAnchor.value}
      overlayClass="search-everywhere-overlay"
      panelClass="surface surface-raised surface-pane-centred search-everywhere-modal"
      panelStyle={paneCentre}
      panelRole="dialog"
      keepMounted
      hiddenClass="search-everywhere-hidden"
    >
        <div class="surface-head search-everywhere-header">
          <SearchField
            class="search-everywhere-field"
            inputRef={inputRef}
            inputClass="search-everywhere-input"
            placeholder="Search everywhere…"
            value={query}
            onInput={(e) => {
              setQuery(e.currentTarget.value);
              setSelectedKey(null);
            }}
            onKeyDown={handleKeyDown}
          >
            {query && (
              <button
                class="icon-btn search-field-clear"
                aria-label="Clear search"
                onClick={() => {
                  setQuery('');
                  setSelectedKey(null);
                  inputRef.current?.focus();
                }}
              >
                <ClearIcon />
              </button>
            )}
          </SearchField>
          <button
            class="icon-btn surface-close search-everywhere-close"
            aria-label="Close search"
            data-tooltip="Close search"
            onClick={close}
          >
            <CloseIcon />
          </button>
        </div>
        <div class="search-everywhere-tabs">
          {CATEGORIES.map(cat => {
            const hits = hitsOnTab(cat.id);
            const label = cat.id === 'all' && !query ? 'Recent' : cat.label;
            return (
              <button
                key={cat.id}
                class={`search-everywhere-tab${cat.id === category ? ' active' : ''}`}
                aria-pressed={cat.id === category}
                aria-label={tabAccessibleName(label, hits)}
                data-empty={hits.kind === 'none' ? '' : undefined}
                onClick={() => setCategory(cat.id)}
              >
                {label}
                {hits.kind === 'some' && <span class="search-everywhere-tab-count">{hits.count}</span>}
              </button>
            );
          })}
        </div>
        <div class="search-everywhere-results" ref={resultsRef}>
          {isRecentsMode ? (
            hasResults ? rows() : <div class="search-everywhere-empty">No recent items</div>
          ) : (
            <LoadingFade
              showSkeleton={showSearchLoading && !hasResults}
              skeleton={<ListSkeletonOf count={5} row={() => <ResultRow />} />}
            >
              {searchBody()}
            </LoadingFade>
          )}
        </div>
    </Overlay>
  );
}
