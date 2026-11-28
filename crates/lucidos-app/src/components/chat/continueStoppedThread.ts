import { continueThread } from '../../api/client';
import { showToast } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { followContinuedThread } from './scrollState';

export const CONTINUE_GUARD_MS = 5000;

const continuingThreadIds = new Set<string>();

export async function continueStoppedThread(threadId: string): Promise<boolean> {
  if (continuingThreadIds.has(threadId)) return false;
  continuingThreadIds.add(threadId);
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
