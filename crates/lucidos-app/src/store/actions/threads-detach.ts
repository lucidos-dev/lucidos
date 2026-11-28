
import { detachThread } from '../../api/threads';
import { showConfirm, showToast, threadMap } from '../store';
import { errorDetail } from '../../utils/errorDetail';

export interface DetachConfirmation {
  title: string;
  message: string;
}

export function detachConfirmation(childTitle: string, parentTitle: string): DetachConfirmation {
  const child = childTitle.trim() || 'this thread';
  const parent = parentTitle.trim() || 'its parent thread';
  return {
    title: 'Move to top level?',
    message: [
      `Move "${child}" out of "${parent}"?`,
      'It keeps running and finishes on its own. Nothing is stopped and no work '
        + 'is lost. You will find it at the top level of your thread list.',
      `"${parent}" stops waiting for it and will not get its result. You cannot undo this.`,
    ].join('\n\n'),
  };
}

export function canMoveToTopLevel(threadId: string): boolean {
  return !!threadMap.value.get(threadId)?.meta.parentThreadId;
}

export async function handleDetachThread(threadId: string): Promise<void> {
  const meta = threadMap.value.get(threadId)?.meta;
  if (!meta?.parentThreadId) return;
  const parentTitle = threadMap.value.get(meta.parentThreadId)?.meta.title
    ?? meta.parentThreadTitle
    ?? '';
  const confirmation = detachConfirmation(meta.title ?? '', parentTitle);
  const ok = await showConfirm(confirmation.message, 'Move out', {
    title: confirmation.title,
    cancelLabel: 'Cancel',
    variant: 'default',
  });
  if (!ok) return;
  try {
    await detachThread(threadId);
  } catch (e) {
    showToast(`Could not move the thread to top level: ${errorDetail(e)}`, 'error');
  }
}
