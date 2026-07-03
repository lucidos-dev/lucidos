import { useState, useRef, useCallback } from 'preact/hooks';
import { useThreadSearch } from './useThreadSearch';
import { threadFilterActive } from '../store/threadFilterActive';

/** Shared state for the threads-pane header (desktop ThreadsHeader and
 *  mobile MobileThreadsHeader). Owns the filter-dropdown open/close state,
 *  the toggle button ref the dropdown anchors against, the search-bar
 *  state from useThreadSearch, and the active-filter signal read. */
export function useThreadsHeaderState() {
  const [filterOpen, setFilterOpen] = useState(false);
  const search = useThreadSearch();
  const toggleRef = useRef<HTMLButtonElement>(null);
  const closeFilter = useCallback(() => setFilterOpen(false), []);
  const filterActive = threadFilterActive.value;
  return {
    filterOpen,
    setFilterOpen,
    toggleRef,
    closeFilter,
    filterActive,
    ...search,
  };
}
