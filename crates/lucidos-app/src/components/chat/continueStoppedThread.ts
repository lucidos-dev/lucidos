import { continueThread } from '../../api/client';
import { showToast } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { followContinuedThread } from './scrollState';

/** How long a Continue holds its guard after the POST lands. The start event's
 *  SSE normally takes the control away first; this only covers a slow one. */
export const CONTINUE_GUARD_MS = 5000;

/** Threads with a Continue in flight. Each press emits its own
 *  `ContinuationRequested`, deduped only by event id. So a second press before
 *  the thread reads running would start a second resume. */
const continuingThreadIds = new Set<string>();

/** Resume a thread whose turn was cut short: by an abort, or by a Stop that
 *  left an incomplete change. Shared by `ContinueButton` and the thread
 *  banner's Continue, so a press on either guards both. Returns false when a
 *  Continue is already in flight, or when it failed, which toasts. */
export async function continueStoppedThread(threadId: string): Promise<boolean> {
  if (continuingThreadIds.has(threadId)) return false;
  continuingThreadIds.add(threadId);
  // A SUBMIT: the agent is expected to respond to it, so it gets the same one
  // reaction a send does. Its turn does not exist yet (the continuation renders
  // as a fresh `ContinuationStarted` exchange), so the landing waits for it.
  // Before the awaited POST, because this is the button's own tap and must not
  // wait on the round trip. See `followSubmit`.
  followContinuedThread();
  try {
    await continueThread(threadId);
  } catch (err) {
    continuingThreadIds.delete(threadId);
    showToast(`Failed to continue: ${errorDetail(err)}`, 'error');
    return false;
  }
  setTimeout(() => continuingThreadIds.delete(threadId), CONTINUE_GUARD_MS);
  return true;
}
