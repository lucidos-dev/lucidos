import { holdThread, resumeThread } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { getThreadDisplaySection, showPrompt, showToast, threadIsHeld } from '../store';
import type { ThreadState } from '../thread-events';

export const HOLD_TOOLTIP =
  'Stop this thread and its sub-threads, and keep their changes out of Review until you resume.';

export function canHoldThread(thread: ThreadState | undefined): thread is ThreadState {
  if (!thread || threadIsHeld(thread)) return false;
  if (thread.meta.state === 'composing' || thread.meta.state === 'discarded') return false;
  return getThreadDisplaySection(thread) !== 'archive';
}

export const MENU_HOLD_REASON = 'Held from the thread menu';

export async function promptAndHoldThread(threadId: string): Promise<void> {
  const reason = await showPrompt(
    "Only your own messages will wake this thread until you resume it. Finished sub-threads, background tasks and triggers won't. The thread and its sub-threads stop now, and their changes leave Review.",
    { title: 'Put this thread on hold', placeholder: 'Why? The strip shows it.', okLabel: 'Hold' },
  );
  if (reason === null) return;
  try {
    await holdThread(threadId, reason.trim() || MENU_HOLD_REASON);
  } catch (err) {
    showToast(`Could not put the thread on hold: ${errorDetail(err)}`, 'error');
  }
}

export async function resumeHeldThread(threadId: string): Promise<void> {
  try {
    await resumeThread(threadId);
  } catch (err) {
    showToast(`Could not resume the thread: ${errorDetail(err)}`, 'error');
  }
}
