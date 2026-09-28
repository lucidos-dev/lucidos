import { useState, useRef, useEffect, useCallback, useMemo } from 'preact/hooks';
import { searchEverywhereOpen, searchEverywhereAnchor, appsList, artifacts, triggers, threadMap, settingsScrollTarget, focusedPane } from '../../store/store';
import { Overlay } from '../shared/Overlay';
import { searchEverywhere, type SearchCategory, type SearchResultItem } from '../../api/client';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { openFilePreview } from '../../store/actions/artifacts';
import { openAppById } from '../../store/actions/apps';
import { openSettingsSubview, switchMenuItem } from '../../store/actions/menu';
import { viewChangeDiffById } from '../../store/actions/repositories';
import { navigateToTrigger } from '../../store/actions/triggers';
import { focusPaneMainControl } from '../layout/paneFocus';
import { searchResultDestinationPane, searchResultIconCategory } from './searchEverywhereActions';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { paneOfFocus, paneUnder, usePaneCentre } from '../../hooks/usePaneCentre';
import { loadedOr, toFailed, type Loadable } from '../../store/types';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText } from '../shared/Skeleton';
import { RECENTS_KEY } from '../../store/actions/entityReferences';
import { SearchIcon, CloseIcon, ClearIcon } from '../shared/icons';
import { CategoryIcon } from '../shared/CategoryIcon';
import { getSettingsSearchResults, findSettingsEntry } from './searchIndex';
import { getMenuSearchResults, findMenuSearchEntry } from './menuIndex';
import './SearchEverywhere.css';

const CATEGORIES: { id: SearchCategory; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'apps', label: 'Apps' },
  { id: 'files', label: 'Files' },
  { id: 'settings', label: 'Settings' },
  { id: 'threads', label: 'Threads' },
  { id: 'triggers', label: 'Triggers' },
  { id: 'changes', label: 'Changes' },
  // Last, because a search is nearly always for a thing rather than for the
  // page it lives on. The pages are a short fixed list, so the tab is where you
  // go to read them all rather than the one you land on.
  { id: 'menu', label: 'Menu' },
];

type LocalCategory = 'settings' | 'menu';

/** The two categories the frontend answers itself, never the engine. */
function isLocalCategory(category: SearchCategory): category is LocalCategory {
  return category === 'settings' || category === 'menu';
}

function localSection(section: LocalCategory, query: string, limit: number): SearchResultItem[] {
  return section === 'settings'
    ? getSettingsSearchResults(query, limit)
    : getMenuSearchResults(query, limit);
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
        return threadMap.value.has(item.id);
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

// `menu` last, for the reason CATEGORIES gives, and so the tab strip and the
// All tab put the pages in the same place.
const SECTION_ORDER = ['apps', 'files', 'settings', 'threads', 'triggers', 'changes', 'menu'];

/** Flatten results by section order into a single indexed list for keyboard navigation. */
function flattenResults(
  results: Record<string, SearchResultItem[]>,
  category: SearchCategory,
): SearchResultItem[] {
  if (category !== 'all') {
    return results[category] ?? [];
  }
  const flat: SearchResultItem[] = [];
  for (const section of SECTION_ORDER) {
    const items = results[section];
    if (!items?.length) continue;
    // In "All" tab, cap at 5 per section
    for (let i = 0; i < Math.min(items.length, 5); i++) {
      flat.push(items[i]);
    }
  }
  return flat;
}

/** Build section boundaries for the "All" tab with precomputed flat index offsets. */
function getSections(results: Record<string, SearchResultItem[]>): { section: string; items: SearchResultItem[]; offset: number }[] {
  const sections: { section: string; items: SearchResultItem[]; offset: number }[] = [];
  let offset = 0;
  for (const section of SECTION_ORDER) {
    const items = results[section];
    if (!items?.length) continue;
    const capped = items.slice(0, 5);
    sections.push({ section, items: capped, offset });
    offset += capped.length;
  }
  return sections;
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

export function SearchEverywhere() {
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState<SearchCategory>('all');
  const [results, setResults] = useState<Loadable<Record<string, SearchResultItem[]>>>({ status: 'not-loaded' });
  const [selectedIndex, setSelectedIndex] = useState(-1);
  const [recents, setRecents] = useState<SearchResultItem[]>([]);
  const inputRef = useRef<HTMLInputElement>(null);
  const resultsRef = useRef<HTMLDivElement>(null);
  const abortRef = useRef<AbortController | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Placeholder rows only once a search has run past the delay gate. Results
  // still show the instant they arrive; the fade only lets the rows go.
  const showSearchLoading = useDelayedLoading(results);

  const isOpen = searchEverywhereOpen.value;
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
    setResults({ status: 'not-loaded' });
    setSelectedIndex(-1);
    if (abortRef.current) abortRef.current.abort();
    if (debounceRef.current) clearTimeout(debounceRef.current);
  }, []);

  // Load recents on open
  useEffect(() => {
    if (isOpen) setRecents(validateRecents(loadRecents()));
  }, [isOpen]);

  // Fire search when query or category changes (skip in recents mode)
  useEffect(() => {
    if (!isOpen) return;
    // Recents need no read. Settle any search still in flight, or its loading
    // state would carry into the next one and skip the delay gate.
    if (!query && category === 'all') {
      setResults({ status: 'not-loaded' });
      return;
    }

    if (isLocalCategory(category)) {
      setResults({ status: 'loaded', data: { [category]: localSection(category, query, 50) } });
      setSelectedIndex(-1);
      return;
    }

    if (debounceRef.current) clearTimeout(debounceRef.current);
    if (abortRef.current) abortRef.current.abort();

    setResults({ status: 'loading' });
    const controller = new AbortController();
    abortRef.current = controller;

    debounceRef.current = setTimeout(async () => {
      try {
        const data = await searchEverywhere(query, category, controller.signal);
        if (!controller.signal.aborted) {
          const merged = category === 'all'
            ? {
                ...data.results,
                settings: localSection('settings', query, 5),
                menu: localSection('menu', query, 5),
              }
            : data.results;
          setResults({ status: 'loaded', data: merged });
          setSelectedIndex(-1);
        }
      } catch (err) {
        if (err instanceof DOMException && err.name === 'AbortError') return;
        // Failed must not read as empty: the list says so where the rows go.
        if (!controller.signal.aborted) setResults(toFailed(err));
      }
    }, 300);

    return () => {
      controller.abort();
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, [query, category, isOpen]);

  // Auto-focus input on open
  useEffect(() => {
    if (isOpen && inputRef.current) {
      inputRef.current.focus();
    }
  }, [isOpen]);

  // Scroll selected result into view
  useEffect(() => {
    if (selectedIndex >= 0 && resultsRef.current) {
      const buttons = resultsRef.current.querySelectorAll('[data-role="search-result"]');
      const el = buttons[selectedIndex] as HTMLElement | undefined;
      el?.scrollIntoView({ block: 'nearest' });
    }
  }, [selectedIndex]);

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

  const isRecentsMode = !query && category === 'all';

  const flat = useMemo(
    () => isRecentsMode ? recents : flattenResults(loadedOr(results, {}), category),
    [isRecentsMode, recents, results, category],
  );
  const sections = useMemo(() => category === 'all' && !isRecentsMode ? getSections(loadedOr(results, {})) : [], [results, category, isRecentsMode]);

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
      return;
    }

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setSelectedIndex(i => Math.min(i + 1, flat.length - 1));
      return;
    }

    if (e.key === 'ArrowUp') {
      e.preventDefault();
      setSelectedIndex(i => Math.max(i - 1, -1));
      return;
    }

    if (e.key === 'Enter' && flat.length > 0) {
      e.preventDefault();
      const idx = selectedIndex >= 0 ? selectedIndex : 0;
      if (flat[idx]) handleSelect(flat[idx]);
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
              key={`${item.category}:${item.id}`}
              item={item}
              index={offset + i}
              selected={offset + i === selectedIndex}
              onSelect={handleSelect}
              onHover={setSelectedIndex}
            />
          ))}
        </div>
      ));
    }
    return flat.map((item, index) => (
      <ResultRow
        key={`${item.category}:${item.id}`}
        item={item}
        index={index}
        selected={index === selectedIndex}
        onSelect={handleSelect}
        onHover={setSelectedIndex}
      />
    ));
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
          <span class="search-everywhere-header-icon"><SearchIcon /></span>
          <input
            ref={inputRef}
            class="search-everywhere-input"
            type="text"
            placeholder="Search everywhere..."
            value={query}
            onInput={(e) => {
              setQuery((e.target as HTMLInputElement).value);
              setSelectedIndex(-1);
            }}
            onKeyDown={handleKeyDown}
          />
          {query && (
            <button
              class="icon-btn search-everywhere-clear"
              aria-label="Clear search"
              onClick={() => {
                setQuery('');
                setSelectedIndex(-1);
                inputRef.current?.focus();
              }}
            >
              <ClearIcon />
            </button>
          )}
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
          {CATEGORIES.map(cat => (
            <button
              key={cat.id}
              class={`search-everywhere-tab${cat.id === category ? ' active' : ''}`}
              aria-pressed={cat.id === category}
              onClick={() => setCategory(cat.id)}
            >
              {cat.id === 'all' && !query ? 'Recent' : cat.label}
            </button>
          ))}
        </div>
        <div class="search-everywhere-results" ref={resultsRef}>
          {isRecentsMode ? (
            hasResults ? rows() : <div class="search-everywhere-empty">No recent items</div>
          ) : results.status === 'failed' ? (
            <div class="search-everywhere-empty error-text">Search failed: {results.error}</div>
          ) : (
            <LoadingFade showSkeleton={showSearchLoading} skeleton={<ListSkeletonOf count={5} row={() => <ResultRow />} />}>
              {results.status === 'loaded' && (hasResults ? rows() : (
                <div class="search-everywhere-empty">
                  {query ? `No results for "${query}"` : `No ${CATEGORIES.find(c => c.id === category)!.label.toLowerCase()}`}
                </div>
              ))}
            </LoadingFade>
          )}
        </div>
    </Overlay>
  );
}
