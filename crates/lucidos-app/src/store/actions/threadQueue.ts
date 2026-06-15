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
  if (!(await showConfirm(`Drop queued spawn "${summary}"? It will not run.`, 'Drop'))) return;
  try {
    await dropThreadQueueEntry(entryId);
    await loadThreadQueue();
  } catch (error) {
    showToast('Failed to drop queue entry: ' + errorDetail(error), 'error');
  }
}

/** Persist a new capacity policy. Returns true on success so the editor can
 *  close only when the save landed. */
export async function saveCapacityPolicy(policy: CapacityPolicy): Promise<boolean> {
  try {
    await putCapacityPolicy(policy);
    await loadThreadQueue();
    showToast('Capacity policy saved', 'info');
    return true;
  } catch (error) {
    showToast('Failed to save capacity policy: ' + errorDetail(error), 'error');
    return false;
  }
}
