import { handleSaveThread, handleUnsaveThread } from '../../store/actions/threads';
import { PinButton } from './PinButton';

/** Pin toggle — pins/unpins a thread (internally the `save` category: `is_saved`
 *  + the `ThreadSaved`/`ThreadUnsaved` events). The toggle maps exactly to the
 *  `resolveThreadActions` save/unsave mapping (`saved ? unsave : save`), so it
 *  can never drift from the selector the close cascade and server-side guards
 *  consult. The unpin path confirms internally (`handleUnsaveThread`). */
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
