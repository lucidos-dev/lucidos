/**
 * What this workspace still owes, as a READ over `releaseNoticeView`.
 *
 * A leaf beside the signal, apart from `actions/releaseNotices.ts`, which
 * loads, resolves, and can send a prompt. The split is not tidiness. That
 * action module reaches compose and chat, and a surface that only wants to know
 * whether anything is owed must not drag those in.
 *
 * The *What's New badge* is what made it matter. Read from the menu drawer, the
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
 *  Drives the modal's "1 of 3" and the *What's New badge*. Everything from the
 *  owed one onward is unanswered by construction, so this is a count of the
 *  tail. It answers `0` for every state but `loaded`, so an unknown list is
 *  never reported as work outstanding. */
export function owedReleaseNoticeCount(): number {
  const view = releaseNoticeView.value;
  if (view.status !== 'loaded') return 0;
  return view.data.notices.filter((n) => !n.resolved).length;
}
