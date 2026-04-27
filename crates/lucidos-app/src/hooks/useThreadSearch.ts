import { useState, useRef, useEffect, useCallback, useMemo } from 'preact/hooks';
import { threadSearchQuery, threadSearchResults } from '../store/store';
import { searchThreads } from '../api/threads';
import { composeHandlers } from '../components/chat/promptFocus';
import { moveHighlight, selectHighlighted } from '../components/drawer/ThreadDrawer';

/** Shared thread search state and handlers for desktop and mobile headers. */
export function useThreadSearch() {
  const [searchOpen, setSearchOpen] = useState(false);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const searchAbortRef = useRef<AbortController | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const doSearch = useCallback((query: string) => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    if (searchAbortRef.current) searchAbortRef.current.abort();

    const trimmed = query.trim();
    if (!trimmed) {
      threadSearchResults.value = { status: 'not-loaded' };
      return;
    }

    debounceRef.current = setTimeout(async () => {
      threadSearchResults.value = { status: 'loading' };
      const controller = new AbortController();
      searchAbortRef.current = controller;
      try {
        const results = await searchThreads(trimmed, controller.signal);
        if (!controller.signal.aborted) {
          threadSearchResults.value = { status: 'loaded', data: results };
        }
      } catch (e) {
        if (!controller.signal.aborted) {
          threadSearchResults.value = { status: 'failed', error: String(e) };
        }
      }
    }, 300);
  }, []);

  const closeSearch = useCallback(() => {
    setSearchOpen(false);
    threadSearchQuery.value = '';
    threadSearchResults.value = { status: 'not-loaded' };
    if (searchAbortRef.current) searchAbortRef.current.abort();
    if (debounceRef.current) clearTimeout(debounceRef.current);
  }, []);

  useEffect(() => () => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    if (searchAbortRef.current) searchAbortRef.current.abort();
  }, []);

  const onSearchInput = useCallback((e: Event) => {
    const val = (e.target as HTMLInputElement).value;
    threadSearchQuery.value = val;
    doSearch(val);
  }, [doSearch]);

  const onSearchKeyDown = useCallback((e: KeyboardEvent) => {
    if (e.key === 'Escape') closeSearch();
    else if (e.key === 'ArrowDown') { e.preventDefault(); moveHighlight(1); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); moveHighlight(-1); }
    else if (e.key === 'Enter') { e.preventDefault(); selectHighlighted(); }
  }, [closeSearch]);

  /**
   * Handlers for the search button. Uses composeHandlers to focus the input
   * synchronously within the user gesture so iOS Safari opens the keyboard.
   * The input is always in the DOM (hidden via CSS when closed), so the ref
   * is populated.
   */
  const openSearchHandlers = useMemo(
    () => composeHandlers(
      () => setSearchOpen(true),
      () => {
        // Expand the search bar BEFORE focusing so iOS calculates the caret
        // from the correct container dimensions. focus() must be synchronous
        // within the gesture (for iOS keyboard), but Preact's state-driven
        // class toggle happens after — by then iOS has locked in the caret
        // size from the 1px hidden container. Adding the class to the DOM
        // first ensures the layout is correct at focus() time.
        const header = searchInputRef.current?.closest('.mobile-threads-header, .threads-header');
        if (header) header.classList.add('search-active');
        searchInputRef.current?.focus();
      },
    ),
    [],
  );

  return { searchOpen, searchInputRef, onSearchInput, onSearchKeyDown, closeSearch, openSearchHandlers };
}
