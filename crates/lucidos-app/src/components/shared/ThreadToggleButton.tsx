import { toggleThreads } from '../../store/actions/pane';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { ThreadsIcon } from './icons';

interface Props {
  class?: string;
}

const LABEL = 'Show or hide thread drawer';

export function ThreadToggleButton({ class: cls }: Props) {
  return (
    <button
      class={`icon-btn header-icon thread-toggle${cls ? ` ${cls}` : ''}`}
      // The toggle is purely show/hide and must not change pane focus. Its host
      // (.pane-header-brand / .collapsed-thread-actions) fires focusPane('thread')
      // on pointerdown; swallow that so a click on the toggle never shifts the
      // focus wash to the thread pane. stopPropagation here is bubble-phase only,
      // so the capture-phase overlay outside-dismiss still closes any open
      // popover (see useAnchoredPopover).
      onPointerDown={(e) => e.stopPropagation()}
      onClick={() => toggleThreads()}
      aria-label={LABEL}
      data-tooltip={tooltipWithShortcut(LABEL, 'toggleThreadDrawer')}
    >
      <ThreadsIcon />
    </button>
  );
}
