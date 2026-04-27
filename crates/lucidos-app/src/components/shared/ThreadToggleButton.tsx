import { attentionThreadCount } from '../../store/store';
import { toggleThreads } from '../../store/actions/pane';
import { ThreadsIcon } from './icons';
import { tooltipWithShortcut } from '../../utils/shortcuts';

export function ThreadToggleButton({ class: cls }: { class?: string }) {
  return (
    <button
      class={`icon-btn header-icon thread-toggle${cls ? ` ${cls}` : ''}`}
      onClick={() => toggleThreads()}
      aria-label="Toggle thread drawer"
      data-tooltip={tooltipWithShortcut('Toggle thread drawer', 'toggleThreadDrawer')}
    >
      <ThreadsIcon />
      {attentionThreadCount.value > 0 && (
        <span class="badge">{attentionThreadCount.value}</span>
      )}
    </button>
  );
}
