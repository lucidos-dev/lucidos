import { signal } from '@preact/signals';
import { archiveAll, archiveAllPreflight } from '../../api/threads';
import { showToast } from '../store';
import { errorDetail } from '../../utils/errorDetail';
import { idBatches } from './archive-all';

export const archivingCompleted = signal<ReadonlySet<string>>(new Set());

export async function handleArchiveCompleted(parent: string): Promise<void> {
  if (archivingCompleted.value.has(parent)) return;
  archivingCompleted.value = new Set([...archivingCompleted.value, parent]);
  const archived = new Set<string>();
  let skipped = 0;
  try {
    const preflight = await archiveAllPreflight(parent).catch((e: unknown) => {
      showToast(`Could not check which sub-threads are complete: ${errorDetail(e)}`, 'error');
      return null;
    });
    if (!preflight) return;
    for (const ids of idBatches(preflight.safe.map(t => t.thread_id))) {
      const result = await archiveAll(ids, parent);
      result.archived.forEach(id => archived.add(id));
      skipped += result.kept.length;
    }
    showToast(`Archived ${archived.size} completed sub-threads. Skipped ${skipped}.`, 'success');
  } catch (e) {
    showToast(`Could not finish archiving sub-threads (${archived.size} archived): ${errorDetail(e)}`, 'error');
  } finally {
    const next = new Set(archivingCompleted.value);
    next.delete(parent);
    archivingCompleted.value = next;
  }
}
