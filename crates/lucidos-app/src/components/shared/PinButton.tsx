import { handlePinThread, handleUnpinThread } from '../../store/actions/threads';
import { PinIcon } from './icons';

export function PinButton({ threadId, pinned, stopPropagation }: { threadId: string; pinned: boolean; stopPropagation?: boolean }) {
  return (
    <button class={`icon-btn ${pinned ? 'pinned' : ''}`}
      onClick={(e) => {
        if (stopPropagation) e.stopPropagation();
        if (pinned) handleUnpinThread(threadId);
        else handlePinThread(threadId);
      }}
      aria-label={pinned ? 'Unpin thread' : 'Pin thread'}
      data-tooltip={pinned ? 'Unpin' : 'Pin'}>
      <PinIcon filled={pinned} />
    </button>
  );
}
