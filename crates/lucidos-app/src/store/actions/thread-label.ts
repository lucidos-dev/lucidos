import { threadMap } from '../store';
import { PENDING_TITLE_PLACEHOLDER } from '../thread-events';

/** Format a thread reference for inline display in toasts and prose.
 *  Returns `“Thread Title”` (curly quotes) when the thread has a real
 *  title, or `'thread'` as a fallback when the title is missing,
 *  pending, or equal to the placeholder.
 *
 *  Lives in its own file (not `threads.ts`) so unit tests can import it
 *  without pulling in `threads.ts`'s Preact-coupled deps (`scrollState`,
 *  `pushThreadNavState`, `useScrollMemory`). */
export function formatThreadLabel(threadId: string): string {
  const title = threadMap.value.get(threadId)?.meta.title;
  return title && title !== PENDING_TITLE_PLACEHOLDER ? `“${title}”` : 'thread';
}
