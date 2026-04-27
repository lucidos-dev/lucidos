import { canGoBackThread, canGoForwardThread, threadNavBack, threadNavForward } from '../../store/actions/thread-navigation';
import { BackIcon, ForwardIcon } from './icons';

export function ThreadNav({ showTooltip }: { showTooltip?: boolean }) {
  return (
    <>
      <button
        class="icon-btn header-icon thread-nav-btn"
        disabled={!canGoBackThread.value}
        onClick={threadNavBack}
        aria-label="Previous thread"
        {...(showTooltip ? { 'data-tooltip': 'Previous thread' } : {})}
      >
        <BackIcon />
      </button>
      <button
        class="icon-btn header-icon thread-nav-btn"
        disabled={!canGoForwardThread.value}
        onClick={threadNavForward}
        aria-label="Next thread"
        {...(showTooltip ? { 'data-tooltip': 'Next thread' } : {})}
      >
        <ForwardIcon />
      </button>
    </>
  );
}
