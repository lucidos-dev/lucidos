import { useRef, useEffect, useCallback, useMemo } from 'preact/hooks';
import { threadSearchQuery, threadSearchResults } from '../store/store';
import { threadSearchOpen, threadSearchSession, openThreadSearch, closeThreadSearch } from '../store/threadSearch';
import { searchThreads } from '../api/threads';
import { toFailed } from '../store/types';
import { composeHandlers } from '../components/chat/composeHandlers';
import { moveHighlight, selectHighlighted } from '../components/drawer/ThreadDrawer';

export function useThreadSearch() {
  const searchOpen = threadSearchOpen.value;
  const searchInputRef = useRef<HTMLInputElement>(null);
  const searchAbortRef = useRef<AbortController | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const cancelPending = useCallback(() => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    if (searchAbortRef.current) searchAbortRef.current.abort();
  }, []);

  const doSearch = useCallback((query: string) => {
    cancelPending();

    const trimmed = query.trim();
    if (!trimmed) {
      threadSearchResults.value = { status: 'not-loaded' };
      return;
    }

    const session = threadSearchSession();
    const stillWanted = (controller?: AbortController) =>
      session === threadSearchSession() && !controller?.signal.aborted;
    debounceRef.current = setTimeout(async () => {
      if (!stillWanted()) return;
      threadSearchResults.value = { status: 'loading' };
      const controller = new AbortController();
      searchAbortRef.current = controller;
      try {
        const results = await searchThreads(trimmed, controller.signal);
        if (stillWanted(controller)) {
          threadSearchResults.value = { status: 'loaded', data: results };
        }
      } catch (e) {
        if (stillWanted(controller)) {
          threadSearchResults.value = toFailed(e);
        }
      }
    }, 300);
  }, [cancelPending]);

  useEffect(() => {
    if (!searchOpen) cancelPending();
  }, [searchOpen, cancelPending]);

  useEffect(() => cancelPending, [cancelPending]);

  const onSearchInput = useCallback((e: Event) => {
    const val = (e.target as HTMLInputElement).value;
    threadSearchQuery.value = val;
    doSearch(val);
  }, [doSearch]);

  const onSearchKeyDown = useCallback((e: KeyboardEvent) => {
    if (e.key === 'Escape') closeThreadSearch();
    else if (e.key === 'ArrowDown') { e.preventDefault(); moveHighlight(1); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); moveHighlight(-1); }
    else if (e.key === 'Enter') { e.preventDefault(); selectHighlighted(); }
  }, []);

  const openSearchHandlers = useMemo(
    () => composeHandlers(
      openThreadSearch,
      () => {
        const header = searchInputRef.current?.closest('.mobile-threads-header, .threads-header');
        if (header) header.classList.add('search-active');
        searchInputRef.current?.focus();
      },
    ),
    [],
  );

  return { searchOpen, searchInputRef, onSearchInput, onSearchKeyDown, closeSearch: closeThreadSearch, openSearchHandlers };
}
