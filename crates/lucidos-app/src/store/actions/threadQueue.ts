import { threadQueue, showToast, showConfirm } from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import type { CapacityPolicy } from '../types';
import {
  getThreadQueue,
  runThreadQueueEntryNow,
  dropThreadQueueEntry,
  putCapacityPolicy,
} from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';

export async function loadThreadQueue(): Promise<void> {
  setLoadingIfFresh(threadQueue);
  try {
    const data = await getThreadQueue();
    threadQueue.value = { status: 'loaded', data };
  } catch (error) {
    threadQueue.value = toFailed(error);
  }
}

/** Whether the thread queue holds a spawn for `threadId`, read fresh from the
 *  engine. Such a thread owns no summary row until it starts. Throws when the
 *  queue cannot be read, so a caller never mistakes that for "not queued". */
export async function threadQueueHolds(threadId: string): Promise<boolean> {
  const data = await getThreadQueue();
  threadQueue.value = { status: 'loaded', data };
  return data.entries.some(entry => entry.thread_id === threadId);
}

/** Force-admit a queued entry, ignoring capacity caps (panel "Run now"). */
export async function runQueueEntryNow(entryId: string): Promise<void> {
  try {
    await runThreadQueueEntryNow(entryId);
    await loadThreadQueue();
  } catch (error) {
    showToast('Failed to run queue entry: ' + errorDetail(error), 'error');
  }
}

/** Drop a queued entry without running it (panel "Drop", with confirm). */
export async function dropQueueEntry(entryId: string, summary: string): Promise<void> {
  if (!(await showConfirm(`Drop queued spawn "${summary}"? It will not run.`, 'Drop', { variant: 'danger' }))) return;
  try {
    await dropThreadQueueEntry(entryId);
    await loadThreadQueue();
  } catch (error) {
    showToast('Failed to drop queue entry: ' + errorDetail(error), 'error');
  }
}

/** Persist a new capacity policy. Self-toasts both outcomes and never rejects. */
export async function saveCapacityPolicy(policy: CapacityPolicy): Promise<void> {
  try {
    await putCapacityPolicy(policy);
    await loadThreadQueue();
    showToast('Capacity policy saved', 'info');
  } catch (error) {
    showToast('Failed to save capacity policy: ' + errorDetail(error), 'error');
  }
}
