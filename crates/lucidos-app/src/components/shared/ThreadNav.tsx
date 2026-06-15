import { canGoBackThread, canGoForwardThread, threadNavBack, threadNavForward } from '../../store/actions/thread-navigation';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { BackIcon, ForwardIcon } from './icons';

interface Props {
  showTooltip?: boolean;
}

export function ThreadNav({ showTooltip }: Props) {
  return (
    <>
      <button
        class="icon-btn header-icon thread-nav-btn"
        disabled={!canGoBackThread.value}
        onClick={() => threadNavBack()}
        aria-label="Previous thread"
        {...(showTooltip ? { 'data-tooltip': tooltipWithShortcut('Previous thread', 'previousThread') } : {})}
      >
        <BackIcon />
      </button>
      <button
        class="icon-btn header-icon thread-nav-btn"
        disabled={!canGoForwardThread.value}
        onClick={() => threadNavForward()}
        aria-label="Next thread"
        {...(showTooltip ? { 'data-tooltip': tooltipWithShortcut('Next thread', 'nextThread') } : {})}
      >
        <ForwardIcon />
      </button>
    </>
  );
}
