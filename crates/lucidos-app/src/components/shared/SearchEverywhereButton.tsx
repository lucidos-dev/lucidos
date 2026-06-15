import { useRef } from 'preact/hooks';
import { searchEverywhereOpen, searchEverywhereAnchor } from '../../store/store';
import { SearchIcon } from './icons';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { composeHandlers } from '../chat/promptFocus';
import { focusSearchInput } from '../search/searchEverywhereActions';

export function SearchEverywhereButton({ showTooltip }: { showTooltip?: boolean }) {
  const ref = useRef<HTMLButtonElement>(null);
  return (
    <button
      ref={ref}
      class="icon-btn header-icon"
      data-role="search-everywhere-toggle"
      {...composeHandlers(
        () => {
          // Record this (visible) button as the dismiss anchor so re-tapping it
          // closes via this handler instead of the outside-pointerdown dismiss.
          searchEverywhereAnchor.value = ref.current;
          searchEverywhereOpen.value = !searchEverywhereOpen.value;
        },
        focusSearchInput,
      )}
      aria-label="Search everywhere"
      data-tooltip={showTooltip ? tooltipWithShortcut('Search', 'searchEverywhere') : undefined}
    >
      <SearchIcon />
    </button>
  );
}
