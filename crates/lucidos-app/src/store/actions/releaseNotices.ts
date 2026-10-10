/**
 * *Release notices*: the one-time instructions a release hands the reader.
 *
 * The modal is a stepper over what this workspace still owes, oldest first, one
 * at a time. A later notice cannot be acted on early because it is not on
 * screen yet, which is the whole ordering guarantee.
 *
 * Answering is explicit: the action button, or Got it. Escape and the X close
 * the modal and resolve nothing, so an unanswered notice returns on the next
 * open. `releaseNoticeDismissed` is what makes that possible without lying to
 * the engine, and it deliberately lives only for this page's life.
 *
 * The READS live in `store/releaseNotices.ts`, and that file says why they are
 * apart from these actions.
 */
import { releaseNoticeView, showToast } from '../store';
import { releaseNoticeDismissed } from '../releaseNotices';
import { toFailed, setLoadingIfFresh } from '../types';
import { releaseNotices, resolveReleaseNotice } from '../../api/client';
import type { ReleaseNotice } from '../../api/client';
import { sendSeededPrompt } from './compose';
import { errorDetail } from '../../utils/errorDetail';

export async function loadReleaseNotices(): Promise<void> {
  setLoadingIfFresh(releaseNoticeView);
  try {
    releaseNoticeView.value = { status: 'loaded', data: await releaseNotices() };
  } catch (error) {
    releaseNoticeView.value = toFailed(error);
  }
}

/** Record that the reader answered `id`, and take the settled list back.
 *
 *  The response IS the new list, so no refetch follows. A failure toasts and
 *  leaves the notice owed: silently treating it as answered would spend the one
 *  time the reader is told. */
export async function resolveReleaseNoticeById(id: string): Promise<void> {
  try {
    releaseNoticeView.value = { status: 'loaded', data: await resolveReleaseNotice(id) };
  } catch (error) {
    showToast(`Failed to answer the release notice: ${errorDetail(error)}`, 'error');
  }
}

/** Act on `notice`: send its sentence as a new message, and answer it.
 *
 *  Closing is not optional. The send lands the reader in a new thread, and an
 *  overlay makes everything behind it inert. A modal left open would cover the
 *  thread they were just sent to. Any notices behind this one return on the
 *  next open, and the What's New panel keeps them reachable meanwhile.
 *
 *  Resolved only when the send actually happened. A failed send leaves the
 *  notice owed, because nothing was started.
 *
 *  An ALREADY answered notice can still be run, from its row on the Release
 *  Notices page. Got it says the reader has read the instruction, not that they
 *  have carried it out, so the action outlives it. Nothing is resolved a second
 *  time: the answer is already recorded, and re-recording it would append an
 *  event saying what the first one said. */
export async function takeReleaseNoticeAction(notice: ReleaseNotice): Promise<void> {
  if (!notice.action_prompt) return;
  const sent = await sendSeededPrompt(notice.action_prompt, 'start that from the release notice');
  if (!sent) return;
  releaseNoticeDismissed.value = true;
  if (notice.resolved) return;
  await resolveReleaseNoticeById(notice.id);
}

/** Got it: answer the notice on screen and step to the next one.
 *
 *  The modal stays up, because the list it reads has already moved on. That is
 *  the stepper: one sitting, in order, rather than one modal per open. */
export async function acknowledgeReleaseNotice(notice: ReleaseNotice): Promise<void> {
  await resolveReleaseNoticeById(notice.id);
}

/** Escape or the X. Closes without answering, so the notice returns. */
export function dismissReleaseNoticeModal(): void {
  releaseNoticeDismissed.value = true;
}
