import { handleSaveThread, handleUnsaveThread } from '../../store/actions/threads';
import { PinIcon } from './icons';

/** Pin toggle — pins/unpins a thread (internally the `save` category: `is_saved`
 *  + the `ThreadSaved`/`ThreadUnsaved` events). A filled pin means pinned, an
 *  outline pin means not. The toggle maps exactly to the `resolveThreadActions`
 *  save/unsave mapping (`saved ? unsave : save`), so it can never drift from the
 *  selector the close cascade and server-side guards consult. The unpin path
 *  confirms internally (`handleUnsaveThread`).
 *
 *  `stopPropagation` is for hosts whose container has its own click handler (the
 *  drawer row's focus-thread `onClick`) — the toggle must not also trigger it. */
export function PinThreadButton({ threadId, saved, stopPropagation, extraClass }: {
  threadId: string;
  saved: boolean;
  stopPropagation?: boolean;
  extraClass?: string;
}) {
  return (
    <button
      type="button"
      class={`icon-btn header-icon pin-thread-btn${saved ? ' active' : ''}${extraClass ? ` ${extraClass}` : ''}`}
      onClick={(e) => {
        if (stopPropagation) e.stopPropagation();
        if (saved) void handleUnsaveThread(threadId);
        else void handleSaveThread(threadId);
      }}
      aria-pressed={saved}
      aria-label={saved ? 'Remove thread from Pinned section' : 'Pin thread'}
      data-tooltip={saved ? 'Pinned — click to unpin' : 'Pin thread'}
    >
      <PinIcon filled={saved} />
    </button>
  );
}
