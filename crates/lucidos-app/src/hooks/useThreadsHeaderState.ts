import { useEffect } from 'preact/hooks';
import { useThreadSearch } from './useThreadSearch';
import { threadFilterActive } from '../store/threadFilterActive';
import { drawerView, attentionThreadCount } from '../store/store';
import { filterButtonState } from '../components/layout/ThreadFilterPanel';
import {
  threadFilterPanelOpen,
  closeThreadFilterPanel,
  toggleThreadFilterPanel,
} from '../store/threadFilterPanel';

/** Shared state for the threads-pane header (desktop ThreadsHeader and
 *  mobile MobileThreadsHeader). Owns the search-bar state from useThreadSearch,
 *  the threads-pane filter panel's toggle, and the whole appearance of the
 *  Filter button it toggles: both headers render the same control, so its glyph
 *  and its active state are derived here rather than twice.
 *
 *  The filter panel's open state is a SIGNAL, not local state: the panel renders
 *  inside the thread drawer pane (`ThreadDrawer`), not in the header, and both
 *  headers instantiate this hook. */
export function useThreadsHeaderState() {
  const search = useThreadSearch();
  const filterOpen = threadFilterPanelOpen.value;

  // One button, two jobs, because it is the panel's only way in AND its only way
  // out: the panel carries no Close button of its own, so while it is open the
  // button drops to a bare X, highlight and badge included (`filterButtonState`
  // owns that whole decision).
  //
  // The accessible NAME stays "Filter threads" either way (the disclosure
  // pattern): `aria-expanded` is what tells a screen reader which way the next
  // press goes, and a name that changed under the user would name the control by
  // what it is about to do rather than by what it is.
  const filterButton = filterButtonState({
    view: drawerView.value,
    panelOpen: filterOpen,
    channelFilterActive: threadFilterActive.value,
    attentionCount: attentionThreadCount.value,
  });

  // Search and the filter panel compete for the same pane body (search swaps the
  // drawer list for its results, the panel covers it), so opening search puts the
  // panel away rather than leaving it over the results the user is typing for.
  const searchOpen = search.searchOpen;
  useEffect(() => {
    if (searchOpen) closeThreadFilterPanel();
  }, [searchOpen]);

  return {
    filterOpen,
    toggleFilter: toggleThreadFilterPanel,
    FilterButtonIcon: filterButton.Icon,
    filterButtonActive: filterButton.active,
    filterButtonBadge: filterButton.badge,
    ...search,
  };
}
