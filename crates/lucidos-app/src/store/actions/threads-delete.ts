
import { ApiError } from '../../api/client';
import { deletePreflight, deleteThreadFamily, type DeletePreflight } from '../../api/threads';
import type { ConfirmDetails } from '../types';
import {
  showConfirm,
  showToast,
  threadMap,
} from '../store';
import { archiveRequestInFlight } from '../rowIntents';
import { errorDetail } from '../../utils/errorDetail';
import { blockedRefusal, blockerFromMembers } from './blockerCopy';
import { collectThreadFamily } from './threadFamily';
import { showBlockedToast } from './threads';
import { resolveThreadActions } from './threadActions';
import { dropDeletedThreads } from './threads-drop';

export interface DeleteConfirmation {
  title: string;
  message: string;
  details?: ConfirmDetails;
}

export function deleteConfirmation(
  preflight: DeletePreflight,
  targetTitle: string,
  canArchive: boolean,
): DeleteConfirmation {
  const subCount = Math.max(0, preflight.thread_count - 1);
  const title =
    preflight.thread_count > 1
      ? `Delete ${preflight.thread_count} threads?`
      : 'Delete this thread?';

  const named = targetTitle.trim() || 'this thread';
  const cascade =
    subCount === 1 ? ' and its sub-thread' : subCount > 1 ? ` and its ${subCount} sub-threads` : '';
  const paragraphs = [
    `This deletes "${named}"${cascade}, with everything said in them.`,
  ];

  if (preflight.memory_count > 0) {
    paragraphs.push(
      'Lucidos will also forget what it learned here. It can lose something '
      + 'another thread taught it, because when two threads say the same thing '
      + 'it keeps only one copy.',
    );
  }
  if (preflight.summary_rebuild_count > 0) {
    const n = preflight.summary_rebuild_count;
    const summaries = n === 1 ? '1 summary tree line' : `${n} summary tree lines`;
    const calls = n === 1 ? '1 background model call' : `${n} background model calls`;
    paragraphs.push(`Lucidos will rebuild ${summaries} this thread fed into, which costs about ${calls}.`);
  }
  if (preflight.has_unapplied_branch_work) {
    paragraphs.push("Work on this thread's branch that you never applied goes too.");
  }
  if (preflight.has_applied_changes) {
    paragraphs.push('Code you already applied stays.');
  }
  if (preflight.backups_present) {
    paragraphs.push('Backups taken before now still hold this.');
  }
  paragraphs.push('This cannot be undone.');
  if (canArchive) {
    const them = subCount > 0 ? 'them' : 'it';
    paragraphs.push(`Archive keeps ${them} instead, in the Archive section, where search still finds ${them}.`);
  }

  const details: ConfirmDetails | undefined =
    preflight.sub_thread_titles.length > 0
      ? {
          groups: [
            {
              header: subCount === 1 ? 'Sub-thread' : `${subCount} sub-threads`,
              items: preflight.sub_thread_titles.map((t) => t.trim() || 'Untitled thread'),
            },
          ],
        }
      : undefined;

  return { title, message: paragraphs.join('\n\n'), details };
}

const DELETE_REFUSED = "Can't delete yet";

export function showDeleteError(err: unknown, threadId: string): void {
  const refusal = blockedRefusal(err, threadId);
  if (refusal) {
    showBlockedToast(DELETE_REFUSED, refusal);
    return;
  }
  showToast(formatDeleteErrorToast(err), 'error');
}

export function formatDeleteErrorToast(err: unknown): string {
  if (err instanceof ApiError && (err.httpCode === 403 || err.httpCode === 401)) {
    return 'Only a signed-in device can delete a thread.';
  }
  return `Failed to delete thread: ${errorDetail(err)}`;
}

export async function handleDeleteThread(threadId: string): Promise<void> {
  if (archiveRequestInFlight(threadId)) return;

  let preflight: DeletePreflight;
  try {
    preflight = await deletePreflight(threadId);
  } catch (e) {
    showDeleteError(e, threadId);
    return;
  }

  const blocked = blockerFromMembers(preflight.blocked_by, threadId);
  if (blocked) {
    showBlockedToast(DELETE_REFUSED, blocked);
    return;
  }

  const archive = resolveThreadActions(threadId).find((a) => a.kind === 'archive');

  const title = threadMap.value.get(threadId)?.meta.title ?? '';
  const confirmation = deleteConfirmation(preflight, title, !!archive);
  let archiveChosen = false;
  const ok = await showConfirm(confirmation.message, 'Delete', {
    title: confirmation.title,
    cancelLabel: 'Cancel',
    variant: 'danger',
    details: confirmation.details,
    extraAction: archive
      ? { label: 'Archive instead', onClick: () => { archiveChosen = true; } }
      : undefined,
  });
  if (archive && archiveChosen) {
    await archive.invoke();
    return;
  }
  if (!ok) return;

  const family = collectThreadFamily(threadId);
  try {
    const result = await deleteThreadFamily(threadId);
    dropDeletedThreads(result.deleted.length > 0 ? result.deleted : [...family]);
  } catch (e) {
    showDeleteError(e, threadId);
  }
}
