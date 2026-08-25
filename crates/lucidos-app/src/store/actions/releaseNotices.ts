/**
 * *Release notices*: the one-time instructions a release hands the reader.
 *
 * The modal is a stepper over what this workspace still owes, oldest first, one
 * at a time. A later notice cannot be acted on early because it is not on
 * screen yet, which is the whole ordering guarantee.
 *
 * Answering is explicit: the action button, or Got it. Escape and the X close
 * the modal and resolve nothing, so an unanswered notice returns on the next
 * open. {@link releaseNoticeDismissed} is what makes that possible without
 * lying to the engine, and it deliberately lives only for this page's life.
 */
import { signal } from '@preact/signals';
import { releaseNoticeView, showToast } from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import { releaseNotices, resolveReleaseNotice } from '../../api/client';
import type { ReleaseNotice } from '../../api/client';
import { sendSeededPrompt } from './compose';
import { errorDetail } from '../../utils/errorDetail';

/** Has the reader closed the modal without answering the notice in it?
 *
 *  Page-local and never persisted. Escape means "not now", and the next open is
 *  when the question is asked again. Persisting it would turn a dismissal into
 *  an answer, which is the one thing the explicit-resolution rule forbids. */
export const releaseNoticeDismissed = signal(false);

/** The notice the modal owes an answer for, or `null`. */
export function owedReleaseNotice(): ReleaseNotice | null {
  const view = releaseNoticeView.value;
  if (view.status !== 'loaded' || !view.data.next_id) return null;
  return view.data.notices.find((n) => n.id === view.data.next_id) ?? null;
}

/** Should the modal be up?
 *
 *  The App-level slot reads this, so the modal's own chunk is fetched only by a
 *  workspace that owes something. That is a minority of loads. The component
 *  re-checks, because it needs the notice itself anyway. */
export function releaseNoticeModalOpen(): boolean {
  return !releaseNoticeDismissed.value && owedReleaseNotice() !== null;
}

/** How many notices are still owed, counting the one on screen.
 *
 *  Drives the modal's "1 of 3". Everything from the owed one onward is
 *  unanswered by construction, so this is a count of the tail. */
export function owedReleaseNoticeCount(): number {
  const view = releaseNoticeView.value;
  if (view.status !== 'loaded') return 0;
  return view.data.notices.filter((n) => !n.resolved).length;
}

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
 *  Resolved only when the send actually happened. A declined draft override
 *  leaves the notice owed, because nothing was started. */
export async function takeReleaseNoticeAction(notice: ReleaseNotice): Promise<void> {
  if (!notice.action_prompt) return;
  const sent = await sendSeededPrompt(notice.action_prompt, 'start that from the release notice');
  if (!sent) return;
  releaseNoticeDismissed.value = true;
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
