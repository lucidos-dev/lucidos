import { signal } from '@preact/signals';
import { releaseNoticeView, showToast } from '../store';
import { releaseNoticeDismissed } from '../releaseNotices';
import { toFailed, setLoadingIfFresh } from '../types';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';
import { releaseNotices, resolveReleaseNotice } from '../../api/client';
import type { ReleaseNoticeAnswer } from '../../api/client';
import { sendSeededPrompt } from './compose';
import { errorDetail } from '../../utils/errorDetail';

export const releaseNoticesSlice: SliceOf<'release_notices'> = {
  begin: () => setLoadingIfFresh(releaseNoticeView),
  apply: (data) => { releaseNoticeView.value = { status: 'loaded', data }; },
  fail: (error) => { releaseNoticeView.value = toFailed(error); },
};

export function loadReleaseNotices(): Promise<void> {
  return loadSlice('release_notices', releaseNoticesSlice, () => releaseNotices());
}

export const releaseNoticeSubmitting = signal(false);

export async function submitReleaseNotice(
  answer: ReleaseNoticeAnswer | null,
  prompt: string | null = null,
): Promise<void> {
  if (releaseNoticeSubmitting.peek()) return;
  releaseNoticeSubmitting.value = true;
  try {
    if (prompt && !await sendSeededPrompt(prompt, 'start the workspace check')) return;
    if (answer) releaseNoticeView.value = {
      status: 'loaded',
      data: await resolveReleaseNotice(answer),
    };
  } catch (error) {
    showToast(`Failed to answer the release notice: ${errorDetail(error)}`, 'error');
  } finally {
    releaseNoticeSubmitting.value = false;
  }
}

export function dismissReleaseNoticeModal(): void {
  releaseNoticeDismissed.value = true;
}
