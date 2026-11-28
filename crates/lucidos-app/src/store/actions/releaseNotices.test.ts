import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { ReleaseNoticeView } from '../../api/client';

const resolveReleaseNotice = vi.fn();
const sendSeededPrompt = vi.fn();
const showToast = vi.fn();

vi.mock('../store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../store')>()),
  showToast: (...a: unknown[]) => showToast(...a),
}));

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  resolveReleaseNotice: (...a: unknown[]) => resolveReleaseNotice(...a),
}));

vi.mock('./compose', () => ({
  sendSeededPrompt: (...a: unknown[]) => sendSeededPrompt(...a),
}));

const { submitReleaseNotice } = await import('./releaseNotices');
const { releaseNoticeAnswer, releaseNoticeModalOpen } = await import('../releaseNotices');
const { releaseNoticeView } = await import('../store');

const settled: ReleaseNoticeView = { releases: [], owed_through: null, action_prompt: null };
const outstanding: ReleaseNoticeView = {
  releases: [{
    since: '1.0.0',
    items: ['a', 'b', 'c'].map((id) => ({
      id, since: '1.0.0', title: id, summary: 'Check it.', body: 'Details.', resolved: id === 'a',
    })),
    resolved: false,
    action_prompt: 'Run the checks.',
  }],
  owed_through: 'c',
  action_prompt: 'Run the checks.',
};

describe('answering release notices', () => {
  beforeEach(() => {
    vi.resetAllMocks();
    releaseNoticeView.value = { status: 'loaded', data: outstanding };
  });

  it('answers the exact visible snapshot and adopts the returned view', async () => {
    resolveReleaseNotice.mockResolvedValue(settled);
    await submitReleaseNotice(releaseNoticeAnswer(outstanding));
    expect(resolveReleaseNotice).toHaveBeenCalledWith({ id: 'c', shown_ids: ['b', 'c'] });
    expect(sendSeededPrompt).not.toHaveBeenCalled();
    expect(releaseNoticeModalOpen()).toBe(false);
  });

  it('sends the combined prompt before acknowledging the displayed items', async () => {
    sendSeededPrompt.mockResolvedValue(true);
    resolveReleaseNotice.mockResolvedValue(settled);
    await submitReleaseNotice(releaseNoticeAnswer(outstanding), outstanding.action_prompt);
    expect(sendSeededPrompt).toHaveBeenCalledWith('Run the checks.', 'start the workspace check');
    expect(sendSeededPrompt.mock.invocationCallOrder[0])
      .toBeLessThan(resolveReleaseNotice.mock.invocationCallOrder[0]);
  });

  it('keeps the notice owed if the check fails to start', async () => {
    sendSeededPrompt.mockResolvedValue(false);
    await submitReleaseNotice(releaseNoticeAnswer(outstanding), outstanding.action_prompt);
    expect(resolveReleaseNotice).not.toHaveBeenCalled();
    expect(releaseNoticeModalOpen()).toBe(true);
  });

  it('keeps the notice owed and reports a failed acknowledgement', async () => {
    resolveReleaseNotice.mockRejectedValue(new Error('offline'));
    await submitReleaseNotice(releaseNoticeAnswer(outstanding));
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('offline'), 'error');
    expect(releaseNoticeModalOpen()).toBe(true);
  });

  it('can run a check from history without answering the current notice', async () => {
    sendSeededPrompt.mockResolvedValue(true);
    await submitReleaseNotice(null, 'Review old changes.');
    expect(sendSeededPrompt).toHaveBeenCalledTimes(1);
    expect(resolveReleaseNotice).not.toHaveBeenCalled();
  });

  it('ignores a second press while it sends and settles the first', async () => {
    sendSeededPrompt.mockResolvedValue(true);
    resolveReleaseNotice.mockResolvedValue(settled);
    const answer = releaseNoticeAnswer(outstanding);
    await Promise.all([
      submitReleaseNotice(answer, outstanding.action_prompt),
      submitReleaseNotice(answer, outstanding.action_prompt),
    ]);
    expect(sendSeededPrompt).toHaveBeenCalledTimes(1);
    expect(resolveReleaseNotice).toHaveBeenCalledTimes(1);
  });
});
