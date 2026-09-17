/** Deleting a thread: the confirmation, the call, and dropping the rows from
 *  the client.
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
  focusedThreadId,
  showConfirm,
  showToast,
  threadMap,
} from '../store';
import { errorDetail } from '../../utils/errorDetail';
import { forgetComposeState } from './compose';
import { collectThreadFamily, focusThread, unfocusThread, visibleCandidatesAround } from './threads';
import { resolveThreadActions } from './threadActions';
import { forgetThreadEventsFailures } from './thread-loading';
import { removeThreadNavEntries } from './thread-navigation';

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

/** What a blocking member's `reason` slug means, as the tail of a sentence. */
const BLOCKER_PHRASE: Record<string, string> = {
  running: 'is still running',
  waiting_for_user_answer: 'is waiting for your answer',
  pending_change: 'has a pending change to apply or discard',
  agent_session_live: 'still has a coding agent running',
};

/** What the user is told when the engine refuses the cascade.
 *
 *  The 409 body is the shape archive answers with, and the reasons overlap, so
 *  a bare "409" (empty `statusText`, no `body.error`) would tell them nothing.
 *
 *  `target` is what makes a one-blocker refusal honest. The engine's `blocking`
 *  list covers the whole family, the target included. Reporting every entry as
 *  a sub-thread told a childless thread one of its sub-threads was busy.
 */
export function formatDeleteErrorToast(err: unknown, target?: string): string {
  if (err instanceof ApiError && err.body && typeof err.body === 'object') {
    const body = err.body as Record<string, unknown>;
    if (err.httpCode === 403 || err.httpCode === 401) {
      return 'Only a signed-in device can delete a thread.';
    }
    if (body.reason === 'descendants_blocking') {
      const blocking = (Array.isArray(body.blocking) ? body.blocking : []) as {
        thread_id?: string;
        title?: string | null;
        reason?: string;
      }[];
      if (blocking.length > 1) {
        return `Can't delete yet, ${blocking.length} threads in this family are still busy`;
      }
      const only = blocking[0];
      const phrase = BLOCKER_PHRASE[only?.reason ?? ''] ?? 'is still busy';
      if (only?.thread_id && only.thread_id === target) {
        return `Can't delete, this thread ${phrase}`;
      }
      const named = only?.title?.trim();
      return named
        ? `Can't delete yet, "${named}" ${phrase}`
        : `Can't delete yet, a sub-thread ${phrase}`;
    }
    if (body.reason === 'parent_not_deletable') {
      return body.parent_status === 'waiting_for_user_answer'
        ? "Can't delete, this thread is waiting for your answer"
        : "Can't delete, this thread is still running";
    }
    if (body.reason === 'parent_has_pending_changes') {
      return "Can't delete, apply or discard the pending change first";
    }
  }
  return `Failed to delete thread: ${errorDetail(err)}`;
}

/** Drop a deleted family from the client.
 *
 *  Idempotent, and called from two places: the delete this device made, and
 *  the `ThreadsDeleted` frame every other device receives. Neither can wait for
 *  the other, and running both is harmless.
 *
 *  The focus hand-off does NOT reveal the thread pane, because deleting is not
 *  navigation. The thread drawer is its own pane on mobile, so a reveal would
 *  swipe the user away on every tap.
 */
export function dropDeletedThreads(ids: readonly string[]): void {
  const gone = new Set(ids);
  if (gone.size === 0) return;

  const focused = focusedThreadId.value;
  // Snapshot the position anchor BEFORE the rows leave the map: once they are
  // gone, `visibleCandidatesAround` cannot find the one the user was on.
  const candidates = focused && gone.has(focused) ? visibleCandidatesAround(focused) : [];

  const next = new Map(threadMap.value);
  for (const id of gone) {
    next.delete(id);
    // A thread that will never be fetched again owes the same cleanup a
    // discarded draft does: nothing may hold a pointer into it. `compose.ts`
    // owns eight private per-thread maps, so it does its own half.
    forgetComposeState(id);
    forgetThreadEventsFailures(id);
    removeThreadNavEntries(id);
  }
  threadMap.value = next;

  if (focused && gone.has(focused)) {
    const nextId = candidates.find((id) => !gone.has(id)) ?? null;
    if (nextId) focusThread(nextId, { revealPane: false });
    else unfocusThread({ revealPane: false });
  }
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
    showToast(formatDeleteErrorToast(e, threadId), 'error');
    return;
  }

  if (preflight.blocked_by.length > 0) {
    const refusal = new ApiError(409, 'blocked', {
      reason: 'descendants_blocking',
      blocking: preflight.blocked_by,
    });
    showToast(formatDeleteErrorToast(refusal, threadId), 'error');
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
    showToast(formatDeleteErrorToast(e, threadId), 'error');
  }
}
