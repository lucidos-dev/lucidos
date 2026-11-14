import { signal } from '@preact/signals';
import { archiveAll, type ArchiveAllPreflight } from '../../api/threads';
import { showToast } from '../store';
import { errorDetail } from '../../utils/errorDetail';
import { idBatches } from './archive-all';

/** Shared across menus so closing and reopening cannot submit twice. */
export const archivingCompleted = signal<ReadonlySet<string>>(new Set());

export async function handleArchiveCompleted(parent: string, preflight: ArchiveAllPreflight): Promise<void> {
  if (archivingCompleted.value.has(parent)) return;
  archivingCompleted.value = new Set([...archivingCompleted.value, parent]);
  const archived = new Set<string>();
  let skipped = 0;
  try {
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
