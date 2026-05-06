import { canGoBackThread, canGoForwardThread, threadNavBack, threadNavForward } from '../../store/actions/thread-navigation';
import { BackIcon, ForwardIcon } from './icons';

interface Props {
  showTooltip?: boolean;
  keepCurrentPane?: boolean;
}

export function ThreadNav({ showTooltip, keepCurrentPane }: Props) {
  const opts = keepCurrentPane ? { skipPaneNav: true } : undefined;
  return (
    <>
      <button
        class="icon-btn header-icon thread-nav-btn"
        disabled={!canGoBackThread.value}
        onClick={() => threadNavBack(opts)}
        aria-label="Previous thread"
        {...(showTooltip ? { 'data-tooltip': 'Previous thread' } : {})}
      >
        <BackIcon />
      </button>
      <button
        class="icon-btn header-icon thread-nav-btn"
        disabled={!canGoForwardThread.value}
        onClick={() => threadNavForward(opts)}
        aria-label="Next thread"
        {...(showTooltip ? { 'data-tooltip': 'Next thread' } : {})}
      >
        <ForwardIcon />
      </button>
    </>
  );
}
