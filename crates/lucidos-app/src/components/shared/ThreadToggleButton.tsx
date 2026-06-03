import { attentionThreadCount } from '../../store/store';
import { toggleThreads } from '../../store/actions/pane';
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
      data-tooltip={ariaLabel}
    >
      <ThreadsIcon />
      {attentionThreadCount.value > 0 && (
        <span class="badge">{attentionThreadCount.value}</span>
      )}
    </button>
  );
}
