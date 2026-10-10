/** Deleting a thread: the confirmation and the call. Dropping the rows from
 *  the client is `threads-drop.ts`.
 *
 *  Separate from `threads.ts` (archive, pin, focus) because the two actions are
 *  deliberately different weights. Archive puts a thread down and keeps it
 *  searchable. Delete removes it, its sub-threads, everything said in them and
 *  what Lucidos learned from them, with no undo. ADR 0192 holds the decision.
 *
 *  **Every line of the dialog is conditional on the preflight.** A warning
 *  about branch work on a chat thread claims something the delete does not do.
 *  So does one about backups on a workspace that never took one.
 *
 *  **The dialog offers Archive beside Delete**, so the reversible way out sits
 *  next to the final one. It appears exactly where the ⋯ menu offers Archive,
 *  so a thread already in the Archive section sees Delete alone.
 */

import { ApiError } from '../../api/client';
import { deletePreflight, deleteThreadFamily, type DeletePreflight } from '../../api/threads';
import type { ConfirmDetails } from '../types';
import {
  archivingThreadIds,
  showConfirm,
  showToast,
  threadMap,
} from '../store';
import { errorDetail } from '../../utils/errorDetail';
import { blockedRefusal, blockerFromMembers } from './blockerCopy';
import { collectThreadFamily } from './threadFamily';
import { showBlockedToast } from './threads';
import { resolveThreadActions } from './threadActions';
import { dropDeletedThreads } from './threads-drop';

/** The dialog, as the three things `showConfirm` takes. Pure, so the whole
 *  conditional-copy table is testable without a dialog or a network. */
export interface DeleteConfirmation {
  title: string;
  message: string;
  details?: ConfirmDetails;
}

/** Build the confirmation for `preflight`, naming `targetTitle`.
 *
 *  One paragraph per blank-line block: `<DialogMessage>` renders each as its
 *  own `<p>`, and a single newline collapses to a space.
 *
 *  Only the first line and the no-undo line always appear. Everything between
 *  is a consequence this particular family actually has, and the closing
 *  Archive line rides `canArchive`: naming a way out the dialog does not offer
 *  is the same lie as a warning that does not apply.
 */
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

  // The sub-thread count expands into the list, so the user can see WHICH
  // threads go rather than only how many.
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

/** The title of a refused delete's toast. */
const DELETE_REFUSED = "Can't delete yet";

/** Tell the user why the delete failed. A cascade refusal names its blocker
 *  (ADR 0378); the owner gate and anything unstructured need their own words. */
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

/** Ask the engine what the delete would take, confirm it, then do it.
 *
 *  The preflight runs before the dialog, so the dialog can only claim what is
 *  true of this family. A refusal surfaces there too, rather than after the
 *  user has confirmed something the engine was never going to do.
 */
export async function handleDeleteThread(threadId: string): Promise<void> {
  if (archivingThreadIds.value.has(threadId)) return;

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

  // Read the alternative off the same tagged actions the ⋯ menu renders, so the
  // dialog can only offer an Archive that is actually available. A thread
  // already in the Archive section has none, and delete is its only way out.
  const archive = resolveThreadActions(threadId).find((a) => a.kind === 'archive');

  const title = threadMap.value.get(threadId)?.meta.title ?? '';
  const confirmation = deleteConfirmation(preflight, title, !!archive);
  let archiveChosen = false;
  const ok = await showConfirm(confirmation.message, 'Delete', {
    title: confirmation.title,
    cancelLabel: 'Cancel',
    variant: 'danger',
    details: confirmation.details,
    // The button only RECORDS the choice, and the archive runs below. Archive
    // opens confirms of its own: a pinned thread, an unsent draft, a live
    // subscription. `ConfirmDialog` closes whatever dialog is up after this
    // handler returns. One opened from here would be the one it closes,
    // answered "no" by the same stroke.
    extraAction: archive
      ? { label: 'Archive instead', onClick: () => { archiveChosen = true; } }
      : undefined,
  });
  if (archive && archiveChosen) {
    await archive.invoke();
    return;
  }
  if (!ok) return;

  // The family is walked client-side so the rows leave the list the moment the
  // engine answers. The `ThreadsDeleted` frame carries the authoritative set
  // and runs the same drop again on every device, this one included.
  const family = collectThreadFamily(threadId);
  try {
    const result = await deleteThreadFamily(threadId);
    dropDeletedThreads(result.deleted.length > 0 ? result.deleted : [...family]);
  } catch (e) {
    showDeleteError(e, threadId);
  }
}
