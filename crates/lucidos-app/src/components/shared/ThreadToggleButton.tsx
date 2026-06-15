import { toggleThreads } from '../../store/actions/pane';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { ThreadsIcon } from './icons';

interface Props {
  class?: string;
  /** Label override — the mobile header surfaces "Show threads" because the
   *  button reveals the drawer over the current pane rather than toggling it
   *  in place. */
  ariaLabel?: string;
}

export function ThreadToggleButton({ class: cls, ariaLabel = 'Toggle thread drawer' }: Props) {
  return (
    <button
      class={`icon-btn header-icon thread-toggle${cls ? ` ${cls}` : ''}`}
      onClick={() => toggleThreads()}
      aria-label={ariaLabel}
      data-tooltip={tooltipWithShortcut(ariaLabel, 'toggleThreadDrawer')}
    >
      <ThreadsIcon />
    </button>
  );
}
