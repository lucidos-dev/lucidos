import { PinIcon } from './icons';

/** The pin toggle every pinnable row and title draws: an outline pin while
 *  unpinned, a filled accent pin (`.pin-btn.active`) while pinned. The host
 *  owns what pinning means and names the button for both states.
 *
 *  `stopPropagation` is for hosts whose container has its own click handler (a
 *  row that opens its thread or app on click): the toggle must not also
 *  trigger it. */
export function PinButton({ pinned, label, tooltip, onToggle, stopPropagation, extraClass, tabIndex }: {
  pinned: boolean;
  label: string;
  tooltip: string;
  onToggle: () => void;
  stopPropagation?: boolean;
  extraClass?: string;
  /** `-1` removes it from the Tab order (the drawer row's mouse-only use, where
   *  the drawer is a single tab stop). Default undefined → natively tabbable. */
  tabIndex?: number;
}) {
  return (
    <button
      type="button"
      tabIndex={tabIndex}
      class={`icon-btn header-icon pin-btn${pinned ? ' active' : ''}${extraClass ? ` ${extraClass}` : ''}`}
      onClick={(e) => {
        if (stopPropagation) e.stopPropagation();
        onToggle();
      }}
      aria-pressed={pinned}
      aria-label={label}
      data-tooltip={tooltip}
    >
      <PinIcon filled={pinned} />
    </button>
  );
}
