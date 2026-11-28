import { signal } from '@preact/signals';
import { releaseNoticeView } from './store';
import type { ReleaseNoticeAnswer, ReleaseNoticeView } from '../api/client';

export const releaseNoticeDismissed = signal(false);

export function releaseNoticeAnswer(view: ReleaseNoticeView): ReleaseNoticeAnswer | null {
  if (!view.owed_through) return null;
  return {
    id: view.owed_through,
    shown_ids: view.releases.flatMap((release) =>
      release.items.filter((item) => !item.resolved).map((item) => item.id)),
  };
}

export function releaseNoticeModalView(): ReleaseNoticeView | null {
  const view = releaseNoticeView.value;
  return !releaseNoticeDismissed.value && view.status === 'loaded' && view.data.owed_through
    ? view.data
    : null;
}

export function releaseNoticeModalOpen(): boolean {
  return releaseNoticeModalView() !== null;
}

export function owedReleaseNoticeCount(): number {
  const view = releaseNoticeView.value;
  return view.status === 'loaded' ? view.data.releases.filter((release) => !release.resolved).length : 0;
}
