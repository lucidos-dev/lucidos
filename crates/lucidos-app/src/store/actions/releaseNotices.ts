/**
 * *Release notices*: the one-time instructions a release hands the reader.
 *
 * The modal only announces them and points at the Release Notices panel, where
 * they are read, answered and acted on. The panel stays put while an action's
 * thread runs, so the notices behind it are not lost.
 *
 * Answering is explicit: the action button, or Got it. Every way of closing the
 * modal resolves nothing, so an unanswered notice announces itself again on
 * the next open. `releaseNoticeDismissed` is what makes that possible without
 * lying to the engine, and it deliberately lives only for this page's life.
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
import { openSettingsSubview } from './menu';
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
  if (!sent || notice.resolved) return;
  await resolveReleaseNoticeById(notice.id);
}

/** Got it: answer the notice, and the next one becomes answerable. */
export async function acknowledgeReleaseNotice(notice: ReleaseNotice): Promise<void> {
  await resolveReleaseNoticeById(notice.id);
}

/** Later, Escape or the X. Closes without answering, so the notices return. */
export function dismissReleaseNoticeModal(): void {
  releaseNoticeDismissed.value = true;
}

/** Read them: close the modal and land on the Release Notices panel. Answers
 *  nothing, so a reader who wanders off still gets the modal next open. */
export function openReleaseNoticesPanel(): void {
  releaseNoticeDismissed.value = true;
  openSettingsSubview('release-notices');
}
