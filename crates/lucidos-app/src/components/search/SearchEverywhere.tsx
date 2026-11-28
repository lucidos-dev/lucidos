import { useState, useRef, useEffect, useCallback, useMemo } from 'preact/hooks';
import { searchEverywhereOpen, searchEverywhereAnchor, appsList, artifacts, triggers, settingsScrollTarget, focusedPane } from '../../store/store';
import { Overlay } from '../shared/Overlay';
import {
  searchEverywhere,
  searchText,
  type SearchCategory,
  type SearchResultItem,
  type ServerSearchCategory,
  type TextSearchMode,
  type TextSearchResponse,
} from '../../api/client';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { handleNavigationRequest } from '../../store/actions/navigation-request';
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
import { RECENTS_KEY } from '../../store/actions/searchRecents';
import { CloseIcon, ClearIcon } from '../shared/icons';
import { SearchField } from '../shared/SearchField';
import { CategoryIcon } from '../shared/CategoryIcon';
import { ListedAppIcon } from '../shared/AppIcon';
import { getSettingsSearchResults, findSettingsEntry } from './searchIndex';
import { getMenuSearchResults, findMenuSearchEntry } from './menuIndex';
import {
  ALL_TAB_LIMIT,
  isTextHit,
  rankedSections,
  tabAccessibleName,
  tabHits,
  textHitItem,
  type PaletteItem,
  type TabHits,
  type TextHitItem,
} from './searchSections';
import './SearchEverywhere.css';

const CATEGORIES: { id: SearchCategory; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'apps', label: 'Apps' },
  { id: 'files', label: 'Files' },
  { id: 'text', label: 'Text' },
  { id: 'settings', label: 'Settings' },
  { id: 'threads', label: 'Threads' },
  { id: 'triggers', label: 'Triggers' },
  { id: 'changes', label: 'Changes' },
  { id: 'menu', label: 'Menu' },
];

const RECENT_LABEL = 'Recent';

type LocalCategory = 'settings' | 'menu';
type ServerSection = Exclude<ServerSearchCategory, 'all'>;
type EngineSection = ServerSection | 'text';

const SERVER_SECTIONS: ServerSection[] = ['apps', 'files', 'threads', 'triggers', 'changes'];

export const SERVER_DEBOUNCE_MS = 150;

export const TEXT_TAB_WINDOW = 100;

function isLocalCategory(category: SearchCategory): category is LocalCategory {
  return category === 'settings' || category === 'menu';
}

function isServerSection(category: SearchCategory): category is ServerSection {
  return category !== 'all' && category !== 'text' && !isLocalCategory(category);
}

function localSection(section: LocalCategory, query: string, limit: number): SearchResultItem[] {
  return section === 'settings'
    ? getSettingsSearchResults(query, limit)
    : getMenuSearchResults(query, limit);
}

function localOverview(query: string): Record<LocalCategory, SearchResultItem[]> {
  return { settings: localSection('settings', query, ALL_TAB_LIMIT), menu: localSection('menu', query, ALL_TAB_LIMIT) };
}

function sectionLabel(section: string): string {
  return CATEGORIES.find(c => c.id === section)?.label ?? section;
}

function itemKey(item: PaletteItem): string {
  return `${item.category}:${item.id}`;
}

function fileRecent(path: string): SearchResultItem {
  return { id: path, title: path.slice(path.lastIndexOf('/') + 1), subtitle: path, category: 'files', score: 1 };
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
        return true;
      case 'settings':
        return findSettingsEntry(item.id) !== undefined;
      case 'menu':
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
          {item && (item.category === 'apps'
            ? <ListedAppIcon appId={item.id} name={item.title} />
            : <CategoryIcon category={searchResultIconCategory(item)} />)}
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

function TextHitRow({ item, index, selected, onSelect, onHover }: {
  item: TextHitItem;
  index: number;
  selected: boolean;
  onSelect: (item: TextHitItem) => void;
  onHover: (index: number) => void;
}) {
  const { hit } = item;
  return (
    <button
      data-role="search-result"
      class={`search-everywhere-result${selected ? ' selected' : ''}`}
      onMouseEnter={() => onHover(index)}
      onClick={() => onSelect(item)}
    >
      <span class="search-everywhere-result-icon">
        <CategoryIcon category={searchResultIconCategory(item)} />
      </span>
      <span class="search-everywhere-result-info">
        <span class="search-everywhere-result-title">
          {hit.before}<mark class="search-match">{hit.matched}</mark>{hit.after}
        </span>
        <span class="search-everywhere-result-subtitle">{hit.path}:{hit.line}</span>
      </span>
    </button>
  );
}

function PendingRow({ sections }: { sections: EngineSection[] }) {
  const names = sections.map(s => sectionLabel(s).toLowerCase()).join(', ');
  return (
    <div class="search-everywhere-pending" role="status">
      <span class="mini-spinner" aria-hidden="true" />
      <span>Searching {names}</span>
    </div>
  );
}

type ServerHits = { search: string; sections: Partial<Record<ServerSection, Loadable<SearchResultItem[]>>> };

const NO_SERVER_HITS: ServerHits = { search: '', sections: {} };
const NO_SECTIONS: ServerSection[] = [];

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

type TextAnswer = { search: string; state: Loadable<TextSearchResponse> };

function useTextHits(
  isOpen: boolean,
  search: string,
  query: string,
  mode: TextSearchMode | null,
): Loadable<TextSearchResponse> | null {
  const [answer, setAnswer] = useState<TextAnswer | null>(null);

  useEffect(() => {
    if (!isOpen) setAnswer(null);
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen || mode === null) return;
    const controller = new AbortController();
    const settle = (state: Loadable<TextSearchResponse>) => {
      if (!controller.signal.aborted) setAnswer({ search, state });
    };
    const timer = setTimeout(() => {
      searchText(query, mode, { signal: controller.signal }).then(
        data => settle({ status: 'loaded', data }),
        err => settle(toFailed(err)),
      );
    }, SERVER_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [search, query, mode, isOpen]);

  if (mode === null) return null;
  return answer?.search === search ? answer.state : { status: 'loading' };
}

function textHitItems(state: Loadable<TextSearchResponse> | null): TextHitItem[] {
  return state?.status === 'loaded' && state.data.status === 'ok' ? state.data.hits.map(textHitItem) : [];
}

function counted(count: number, one: string, many: string): string {
  return `${count.toLocaleString()} ${count === 1 ? one : many}`;
}

export function SearchEverywhere() {
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState<SearchCategory>('all');
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [recents, setRecents] = useState<SearchResultItem[]>([]);
  const inputRef = useRef<HTMLInputElement>(null);
  const resultsRef = useRef<HTMLDivElement>(null);
  const drawnSections = useRef<{ query: string; order: string[] }>({ query: '', order: [] });

  const isRecentsMode = !query && category === 'all';
  const isOpen = searchEverywhereOpen.value;
  const overviewState = useServerHits(isOpen, query, query, query ? SERVER_SECTIONS : NO_SECTIONS, ALL_TAB_LIMIT);
  const overviewText = useTextHits(isOpen, query, query, query ? 'preview' : null);
  const pageSections = useMemo(
    () => (isServerSection(category) ? [category] : NO_SECTIONS),
    [category],
  );
  const pageState = useServerHits(isOpen, `${category}\n${query}`, query, pageSections, undefined);
  const pageText = useTextHits(isOpen, `text\n${query}`, query, category === 'text' ? 'all' : null);

  const requested = category === 'all' ? (query ? SERVER_SECTIONS : NO_SECTIONS) : pageSections;
  const listState = category === 'all' ? overviewState : pageState;
  const textState = category === 'all' ? overviewText : category === 'text' ? pageText : null;
  const pending: EngineSection[] = requested.filter(s => listState(s).status === 'loading');
  if (textState?.status === 'loading') pending.push('text');
  const failed: { section: EngineSection; error: string }[] = requested.flatMap(s => {
    const state = listState(s);
    return state.status === 'failed' ? [{ section: s, error: state.error }] : [];
  });
  if (textState?.status === 'failed') failed.push({ section: 'text', error: textState.error });
  const requestedCount = requested.length + (textState ? 1 : 0);
  const textAnswer = textState?.status === 'loaded' ? textState.data : null;
  const showSearchLoading = useDelayedFlag(pending.length > 0);

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

  useEffect(() => {
    if (isOpen && inputRef.current) {
      inputRef.current.focus();
    }
  }, [isOpen]);

  function handleSelect(item: PaletteItem) {
    saveRecent(isTextHit(item) ? fileRecent(item.hit.path) : item);
    close();
    const focusDest = () => focusPaneMainControl(searchResultDestinationPane(item.category));
    if (isTextHit(item)) {
      void handleNavigationRequest(
        { target: 'file', file_path: item.hit.path, line: item.hit.line },
        { find: query },
      );
      return;
    }
    switch (item.category) {
      case 'threads': focusThreadOrBootstrap(item.id); focusDest(); break;
      case 'files': openFilePreview(item.id); focusDest(); break;
      case 'apps': void openAppById(item.id).then(focusDest); break;
      case 'triggers': void navigateToTrigger(item.id).then(focusDest); break;
      case 'settings': {
        const entry = findSettingsEntry(item.id);
        if (!entry) break;
        openSettingsSubview(entry.subview);
        if (entry.anchor) settingsScrollTarget.value = entry.anchor;
        else focusDest();
        break;
      }
      case 'changes': void viewChangeDiffById(item.id).then(focusDest); break;
      case 'menu': {
        const entry = findMenuSearchEntry(item.id);
        if (!entry) break;
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

  const textHits = useMemo(() => textHitItems(textState), [textState]);
  const textFiles = useMemo(() => new Set(textHits.map(item => item.hit.path)).size, [textHits]);
  if (drawnSections.current.query !== query) drawnSections.current = { query, order: [] };
  const sections = category === 'all' && !isRecentsMode
    ? rankedSections(results, query, textHits, drawnSections.current.order)
    : [];
  if (sections.length) drawnSections.current.order = sections.map(s => s.section);
  let flat: PaletteItem[];
  if (isRecentsMode) flat = recents;
  else if (category === 'all') flat = sections.flatMap(s => s.items);
  else if (category === 'text') flat = textHits;
  else flat = results[category] ?? [];

  function hitsOnTab(tab: SearchCategory): TabHits {
    if (tab === 'all' || !overviewLocal) return 'unknown';
    if (tab === 'text') {
      return overviewText?.status === 'loaded' && overviewText.data.status === 'ok'
        ? tabHits({ status: 'loaded', data: overviewText.data.hits })
        : 'unknown';
    }
    return isLocalCategory(tab)
      ? tabHits({ status: 'loaded', data: overviewLocal[tab] })
      : tabHits(overviewState(tab));
  }

  const [shownRows, setShownRows] = useState(TEXT_TAB_WINDOW);
  useEffect(() => setShownRows(TEXT_TAB_WINDOW), [query, category]);
  const rendered = category === 'text' ? flat.slice(0, shownRows) : flat;
  function growOnScroll(e: Event) {
    const list = e.currentTarget as HTMLElement;
    const nearEnd = list.scrollHeight - list.scrollTop - list.clientHeight < list.clientHeight;
    if (category === 'text' && nearEnd && shownRows < flat.length) setShownRows(n => n + TEXT_TAB_WINDOW);
  }
  const selectedIndex = selectedKey === null ? -1 : flat.findIndex(item => itemKey(item) === selectedKey);
  const selectAt = (index: number) => setSelectedKey(index >= 0 && flat[index] ? itemKey(flat[index]) : null);

  useEffect(() => {
    if (category === 'text' && selectedIndex >= shownRows) {
      setShownRows(Math.ceil((selectedIndex + 1) / TEXT_TAB_WINDOW) * TEXT_TAB_WINDOW);
    }
  }, [category, selectedIndex, shownRows]);

  const scrolledToIndex = useRef(-1);
  useEffect(() => {
    if (selectedIndex < 0) {
      scrolledToIndex.current = -1;
      return;
    }
    if (scrolledToIndex.current === selectedIndex || !resultsRef.current) return;
    const buttons = resultsRef.current.querySelectorAll('[data-role="search-result"]');
    const el = buttons[selectedIndex] as HTMLElement | undefined;
    if (!el) return;
    el.scrollIntoView({ block: 'nearest' });
    scrolledToIndex.current = selectedIndex;
  }, [selectedIndex, shownRows]);

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

  function row(item: PaletteItem, index: number) {
    const props = { index, selected: index === selectedIndex, onSelect: handleSelect, onHover: selectAt };
    return isTextHit(item)
      ? <TextHitRow key={itemKey(item)} item={item} {...props} />
      : <ResultRow key={itemKey(item)} item={item} {...props} />;
  }

  function rows() {
    if (category === 'all' && !isRecentsMode) {
      return sections.map(({ section, items, offset }) => (
        <div key={section}>
          <div class="search-everywhere-section-header">{section}</div>
          {items.map((item, i) => row(item, offset + i))}
        </div>
      ));
    }
    return rendered.map(row);
  }

  function textSummary() {
    if (category !== 'text' || !hasResults) return null;
    return (
      <div class="search-everywhere-summary" role="status">
        {counted(textHits.length, 'match', 'matches')} in {counted(textFiles, 'file', 'files')}
      </div>
    );
  }

  function textNotes() {
    if (textAnswer?.status !== 'ok') return null;
    const showSkipped = textAnswer.skipped_large_files > 0 && (category === 'text' || !textHits.length);
    return (
      <>
        {category === 'text' && textAnswer.truncated && (
          <div class="search-everywhere-note">
            Showing the first {counted(textHits.length, 'match', 'matches')}. Refine your query to see the rest.
          </div>
        )}
        {showSkipped && (
          <div class="search-everywhere-note">
            Text search skipped {counted(textAnswer.skipped_large_files, 'large file', 'large files')}.
          </div>
        )}
      </>
    );
  }

  function emptyMessage(): string {
    if (category === 'text' && textAnswer?.status === 'query-too-short') {
      return `Type at least ${textAnswer.min_query_chars} characters to search inside files`;
    }
    return query ? `No results for "${query}"` : `No ${sectionLabel(category).toLowerCase()}`;
  }

  function searchBody() {
    if (!hasResults && pending.length > 0) return null;
    if (!hasResults && failed.length > 0 && failed.length === requestedCount) {
      return <div class="search-everywhere-empty error-text">Search failed: {failed[0].error}</div>;
    }
    return (
      <>
        {textSummary()}
        {hasResults ? rows() : <div class="search-everywhere-empty">{emptyMessage()}</div>}
        {textNotes()}
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
            const label = cat.id === 'all' && !query ? RECENT_LABEL : cat.label;
            return (
              <button
                key={cat.id}
                class={`search-everywhere-tab${cat.id === category ? ' active' : ''}`}
                aria-pressed={cat.id === category}
                aria-label={tabAccessibleName(label, hits)}
                data-empty={hits === 'none' ? '' : undefined}
                onClick={() => setCategory(cat.id)}
              >
                {cat.id === 'all' ? (
                  <span class="search-everywhere-widest" data-widest={RECENT_LABEL}><span>{label}</span></span>
                ) : label}
              </button>
            );
          })}
        </div>
        <div class="search-everywhere-results" ref={resultsRef} onScroll={growOnScroll}>
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
