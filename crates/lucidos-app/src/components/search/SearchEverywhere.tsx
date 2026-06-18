import { useState, useRef, useEffect, useCallback, useMemo } from 'preact/hooks';
import { searchEverywhereOpen, searchEverywhereAnchor, showToast, appsList, artifacts, triggers, threadMap, settingsScrollTarget } from '../../store/store';
import { Overlay } from '../shared/Overlay';
import { searchEverywhere, type SearchCategory, type SearchResultItem } from '../../api/client';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { openFilePreview } from '../../store/actions/artifacts';
import { openAppById } from '../../store/actions/apps';
import { switchMenuItem, openSettingsSubview } from '../../store/actions/menu';
import { viewChangeDiffById } from '../../store/actions/repositories';
import { navigateToTrigger } from '../../store/actions/triggers';
import { RECENTS_KEY } from '../../store/actions/entityReferences';
import { SearchIcon, CloseIcon, ClearIcon } from '../shared/icons';
import { CategoryIcon } from '../shared/CategoryIcon';
import { getSettingsSearchResults, findSettingsEntry } from './searchIndex';
import { errorDetail } from '../../utils/errorDetail';
import './SearchEverywhere.css';

const CATEGORIES: { id: SearchCategory; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'apps', label: 'Apps' },
  { id: 'files', label: 'Files' },
  { id: 'settings', label: 'Settings' },
  { id: 'threads', label: 'Threads' },
  { id: 'triggers', label: 'Triggers' },
  { id: 'changes', label: 'Changes' },
];

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

const SECTION_ORDER = ['apps', 'files', 'settings', 'threads', 'triggers', 'changes'];

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

function ResultRow({ item, index, selected, onSelect, onHover }: {
  item: SearchResultItem;
  index: number;
  selected: boolean;
  onSelect: (item: SearchResultItem) => void;
  onHover: (index: number) => void;
}) {
  return (
    <button
      data-role="search-result"
      class={`search-everywhere-result${selected ? ' selected' : ''}`}
      onMouseEnter={() => onHover(index)}
      onClick={() => onSelect(item)}
    >
      <span class="search-everywhere-result-icon">
        <CategoryIcon category={item.category} />
      </span>
      <span class="search-everywhere-result-info">
        <span class="search-everywhere-result-title">{item.title}</span>
        {item.subtitle && <span class="search-everywhere-result-subtitle">{item.subtitle}</span>}
      </span>
    </button>
  );
}

export function SearchEverywhere() {
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState<SearchCategory>('all');
  const [results, setResults] = useState<Record<string, SearchResultItem[]>>({});
  const [selectedIndex, setSelectedIndex] = useState(-1);
  const [loading, setLoading] = useState(false);
  const [recents, setRecents] = useState<SearchResultItem[]>([]);
  const inputRef = useRef<HTMLInputElement>(null);
  const resultsRef = useRef<HTMLDivElement>(null);
  const abortRef = useRef<AbortController | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const isOpen = searchEverywhereOpen.value;

  const close = useCallback(() => {
    searchEverywhereOpen.value = false;
    setQuery('');
    setCategory('all');
    setResults({});
    setSelectedIndex(-1);
    setLoading(false);
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
    if (!query && category === 'all') {
      setLoading(false);
      return;
    }

    if (category === 'settings') {
      setResults({ settings: getSettingsSearchResults(query, 50) });
      setSelectedIndex(-1);
      setLoading(false);
      return;
    }

    if (debounceRef.current) clearTimeout(debounceRef.current);
    if (abortRef.current) abortRef.current.abort();

    setLoading(true);
    const controller = new AbortController();
    abortRef.current = controller;

    debounceRef.current = setTimeout(async () => {
      try {
        const data = await searchEverywhere(query, category, controller.signal);
        if (!controller.signal.aborted) {
          const merged = category === 'all'
            ? { ...data.results, settings: getSettingsSearchResults(query, 5) }
            : data.results;
          setResults(merged);
          setSelectedIndex(-1);
        }
      } catch (err) {
        if (err instanceof DOMException && err.name === 'AbortError') return;
        if (!controller.signal.aborted) {
          setResults({});
          showToast(`Search failed: ${errorDetail(err)}`, 'error');
        }
      } finally {
        if (!controller.signal.aborted) {
          setLoading(false);
        }
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
    switch (item.category) {
      case 'threads': focusThreadOrBootstrap(item.id); break;
      case 'files': openFilePreview(item.id); break;
      case 'apps': void openAppById(item.id); break;
      case 'triggers': void navigateToTrigger(item.id); break;
      case 'settings': {
        const entry = findSettingsEntry(item.id);
        if (!entry) break;
        switchMenuItem('settings');
        openSettingsSubview(entry.subview);
        if (entry.anchor) settingsScrollTarget.value = entry.anchor;
        break;
      }
      case 'changes': void viewChangeDiffById(item.id); break;
    }
  }

  const isRecentsMode = !query && category === 'all';

  const flat = useMemo(
    () => isRecentsMode ? recents : flattenResults(results, category),
    [isRecentsMode, recents, results, category],
  );
  const sections = useMemo(() => category === 'all' && !isRecentsMode ? getSections(results) : [], [results, category, isRecentsMode]);

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
  return (
    <Overlay
      open={isOpen}
      onClose={close}
      anchor={searchEverywhereAnchor.value}
      overlayClass="search-everywhere-overlay"
      panelClass="search-everywhere-modal"
      panelRole="dialog"
      keepMounted
      hiddenClass="search-everywhere-hidden"
    >
        <div class="search-everywhere-header">
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
            class="icon-btn search-everywhere-close"
            aria-label="Close search"
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
              onClick={() => setCategory(cat.id)}
            >
              {cat.id === 'all' && !query ? 'Recent' : cat.label}
            </button>
          ))}
        </div>
        <div class="search-everywhere-results" ref={resultsRef}>
          {loading && !isRecentsMode ? (
            <div class="search-everywhere-loading"><span class="mini-spinner" /></div>
          ) : !hasResults ? (
            <div class="search-everywhere-empty">
              {query ? `No results for "${query}"` : category === 'all' ? 'No recent items' : `No ${CATEGORIES.find(c => c.id === category)!.label.toLowerCase()}`}
            </div>
          ) : isRecentsMode ? (
            flat.map((item, index) => (
              <ResultRow
                key={`${item.category}:${item.id}`}
                item={item}
                index={index}
                selected={index === selectedIndex}
                onSelect={handleSelect}
                onHover={setSelectedIndex}
              />
            ))
          ) : category === 'all' ? (
            sections.map(({ section, items, offset }) => (
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
            ))
          ) : (
            flat.map((item, index) => (
              <ResultRow
                key={`${item.category}:${item.id}`}
                item={item}
                index={index}
                selected={index === selectedIndex}
                onSelect={handleSelect}
                onHover={setSelectedIndex}
              />
            ))
          )}
        </div>
    </Overlay>
  );
}
