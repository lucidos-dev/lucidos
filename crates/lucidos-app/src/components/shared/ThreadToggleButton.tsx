import { toggleThreads } from '../../store/actions/pane';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { shortcutDef } from '../../utils/shortcuts';
import { ThreadsIcon } from './icons';

interface Props {
  class?: string;
}

/** Same wording as the ⌘⇧1 registry entry, derived rather than copied so a
 *  glossary-driven rename of the shortcut label can't leave this button's
 *  tooltip and aria-label on the old name while Settings shows the new one. */
const LABEL = shortcutDef('toggleThreadDrawer').label;

export function ThreadToggleButton({ class: cls }: Props) {
  return (
    <button
      class={`icon-btn header-icon thread-toggle${cls ? ` ${cls}` : ''}`}
      // The toggle is purely show/hide and must not change pane focus. Its hosts
      // fire focusPane via different events: the header regions
      // (.pane-header-brand / .collapsed-thread-actions) on CLICK, and the thread
      // pane body (.pane-thread, via SplitLayout) on POINTERDOWN. Swallow BOTH so
      // toggling the drawer never shifts pane focus to the thread pane.
      // stopPropagation is bubble-phase only, so the capture-phase overlay
      // outside-dismiss still closes any open popover (see useAnchoredPopover).
      onPointerDown={(e) => e.stopPropagation()}
      onClick={(e) => { e.stopPropagation(); toggleThreads(); }}
      aria-label={LABEL}
      data-tooltip={tooltipWithShortcut(LABEL, 'toggleThreadDrawer')}
    >
      <ThreadsIcon />
    </button>
  );
}
