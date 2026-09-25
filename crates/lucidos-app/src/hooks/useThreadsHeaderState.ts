import { useEffect } from 'preact/hooks';
import { useThreadSearch } from './useThreadSearch';
import { closeThreadFilterPanel } from '../store/threadFilterPanel';

/** Shared state for the threads-pane header (desktop ThreadsHeader and
 *  mobile MobileThreadsHeader): the search-bar state from useThreadSearch.
 *  The Filter button and the pane title read their own state
 *  (`ThreadFilterButton`, `ThreadsPaneTitle`). */
export function useThreadsHeaderState() {
  const search = useThreadSearch();

  // Search and the filter panel compete for the same pane body (search swaps the
  // drawer list for its results, the panel covers it), so opening search puts the
  // panel away rather than leaving it over the results the user is typing for.
  const searchOpen = search.searchOpen;
  useEffect(() => {
    if (searchOpen) closeThreadFilterPanel();
  }, [searchOpen]);

  return search;
}
