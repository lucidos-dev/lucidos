/**
 * What this workspace still owes, as a READ over `releaseNoticeView`.
 *
 * A leaf beside the signal, apart from `actions/releaseNotices.ts`, which
 * loads, resolves, and can send a prompt. The split is not tidiness. That
 * action module reaches compose and chat, and a surface that only wants to know
 * whether anything is owed must not drag those in.
 *
 * The *System attention badge* is what made it matter. Read from the menu drawer, the
 * action module widened the existing import cycle (`pane` to `Drawer` to
 * `menu`) until it reached `connection`. A cycle through a mocked module hands
 * its importers the real one, which is what
 * `store/__tests__/restart-reconnect.test.ts` catches.
 */
import { signal } from '@preact/signals';
import { releaseNoticeView } from './store';
import type { ReleaseNotice } from '../api/client';

/** Has the reader closed the modal without answering the notice in it?
 *
 *  Page-local and never persisted. Escape means "not now", and the next open is
 *  when the question is asked again. Persisting it would turn a dismissal into
 *  an answer, which is the one thing the explicit-resolution rule forbids. */
export const releaseNoticeDismissed = signal(false);

/** The notice the workspace must answer next, or `null`. */
export function owedReleaseNotice(): ReleaseNotice | null {
  const view = releaseNoticeView.value;
  if (view.status !== 'loaded' || !view.data.next_id) return null;
  return view.data.notices.find((n) => n.id === view.data.next_id) ?? null;
}

/** The action button a notice's row draws, or `null` for none.
 *
 *  A deferred action belongs to a later owed notice. The reader runs it once,
 *  at the end, rather than once per notice. An answered notice keeps its
 *  action, because Got it says "read", never "done". */
export function releaseNoticeActionLabel(notice: ReleaseNotice): string | null {
  if (notice.action_deferred && !notice.resolved) return null;
  return notice.action_label ?? null;
}

/** Should the modal be up?
 *
 *  The App-level slot reads this, so the modal's own chunk is fetched only by a
 *  workspace that owes something. That is a minority of loads. */
export function releaseNoticeModalOpen(): boolean {
  return !releaseNoticeDismissed.value && owedReleaseNotice() !== null;
}

/** The titles the modal lists: every unanswered notice, in the order the
 *  panel asks for them. */
export function owedReleaseNoticeTitles(): string[] {
  const view = releaseNoticeView.value;
  if (view.status !== 'loaded') return [];
  return view.data.notices.filter((n) => !n.resolved).map((n) => n.title);
}

/** How many notices are still owed.
 *
 *  Drives the modal's heading and the *System attention badge*. It answers `0`
 *  for every state but `loaded`, so an unknown list is never reported as work
 *  outstanding. */
export function owedReleaseNoticeCount(): number {
  return owedReleaseNoticeTitles().length;
}
