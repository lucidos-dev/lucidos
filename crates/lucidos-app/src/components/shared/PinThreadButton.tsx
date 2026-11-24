import { handleSaveThread, handleUnsaveThread } from '../../store/actions/threads';
import { PinButton } from './PinButton';

export function PinThreadButton({ threadId, saved, stopPropagation, extraClass, tabIndex }: {
  threadId: string;
  saved: boolean;
  stopPropagation?: boolean;
  extraClass?: string;
  tabIndex?: number;
}) {
  return (
    <PinButton
      pinned={saved}
      label={saved ? 'Remove thread from Pinned section' : 'Pin thread'}
      tooltip={saved ? 'Pinned. Click to unpin' : 'Pin thread'}
      onToggle={() => {
        if (saved) void handleUnsaveThread(threadId);
        else void handleSaveThread(threadId);
      }}
      stopPropagation={stopPropagation}
      extraClass={extraClass}
      tabIndex={tabIndex}
    />
  );
}
