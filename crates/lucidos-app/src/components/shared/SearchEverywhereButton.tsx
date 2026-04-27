import { searchEverywhereOpen } from '../../store/store';
import { SearchIcon } from './icons';
import { tooltipWithShortcut } from '../../utils/shortcuts';
import { composeHandlers } from '../chat/promptFocus';
import { focusSearchInput } from '../search/SearchEverywhere';

export function SearchEverywhereButton({ showTooltip }: { showTooltip?: boolean }) {
  return (
    <button
      class="icon-btn header-icon"
      data-role="search-everywhere-toggle"
      {...composeHandlers(
        () => { searchEverywhereOpen.value = !searchEverywhereOpen.value; },
        focusSearchInput,
      )}
      aria-label="Search everywhere"
      data-tooltip={showTooltip ? tooltipWithShortcut('Search', 'searchEverywhere') : undefined}
    >
      <SearchIcon />
    </button>
  );
}
