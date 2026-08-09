import { describe, it, expect, beforeEach, vi } from 'vitest';

const showToast = vi.fn();
const apiAnswerThreadQuestion = vi.fn(async () => true);
const scrollToBottom = vi.fn();
const markThreadAnswering = vi.fn();
const clearThreadAnswering = vi.fn();
const markThreadRerenderStart = vi.fn();
const clearThreadRerenderStart = vi.fn();
// Focused thread is 't1' (the id the tests answer on) so the perf re-render mark
// is stamped on the focused path.
const focusedThreadId = { value: 't1' };

class ApiError extends Error {
  constructor(public readonly httpCode: number, public readonly reason: string) {
    super(`${httpCode} ${reason}`);
  }
}

vi.mock('../store', () => ({ showToast, markThreadAnswering, clearThreadAnswering, focusedThreadId }));
vi.mock('../../api/client', () => ({
  answerThreadQuestion: apiAnswerThreadQuestion,
  ApiError,
}));
vi.mock('../../components/chat/scrollState', () => ({ scrollToBottom }));
vi.mock('../../utils/threadOpenMarks', () => ({ markThreadRerenderStart, clearThreadRerenderStart }));
vi.mock('../../utils/renderPhaseTimers', () => ({ currentPerfBaseline: () => ({ start: 0, md: 0, link: 0 }) }));
vi.mock('./thread-sync', () => ({}));
vi.mock('./threads', () => ({}));

const { answerThreadQuestion } = await import('./chat-claude-code');

describe('answerThreadQuestion', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiAnswerThreadQuestion.mockResolvedValue(true);
  });

  it('sends the answer without moving the transcript', async () => {
    // This used to pin to the bottom before the POST, so the answered card's
    // height-shrink could not unstick the tail and the resumed stream would
    // follow. Answering is not a request to go to the live edge: the resumed
    // stream grows below the reader, and the chevron is how they follow it.
    await answerThreadQuestion('t1', 'tool-use-1', { kind: 'Selected', option_id: 'opt-a' });

    expect(apiAnswerThreadQuestion).toHaveBeenCalledTimes(1);
    expect(scrollToBottom).not.toHaveBeenCalled();
  });

  it('optimistically marks the thread answering before the request, and does NOT clear on success', async () => {
    await answerThreadQuestion('t1', 'tool-use-1', { kind: 'Selected', option_id: 'opt-a' });

    expect(markThreadAnswering).toHaveBeenCalledWith('t1');
    const markOrder = markThreadAnswering.mock.invocationCallOrder[0];
    const apiOrder = apiAnswerThreadQuestion.mock.invocationCallOrder[0];
    expect(markOrder).toBeLessThan(apiOrder); // optimism is set before the await
    // On success the PromptInput effect clears it once status leaves
    // waiting_for_user_answer — the action must NOT clear it itself.
    expect(clearThreadAnswering).not.toHaveBeenCalled();
    // Perf: the re-render mark is stamped (focused thread) and NOT cleared on success.
    expect(markThreadRerenderStart).toHaveBeenCalledWith('t1', expect.objectContaining({ cause: 'answer' }));
    expect(clearThreadRerenderStart).not.toHaveBeenCalled();
  });

  it('clears the optimistic answering flag on a 409 (stale/duplicate, no resume coming)', async () => {
    apiAnswerThreadQuestion.mockResolvedValueOnce(false);
    const ok = await answerThreadQuestion('t1', 'tool-use-1', { kind: 'Selected', option_id: 'opt-a' });

    expect(ok).toBe(false);
    expect(markThreadAnswering).toHaveBeenCalledWith('t1');
    expect(clearThreadAnswering).toHaveBeenCalledWith('t1');
    // Perf: no resume render is coming → the stale re-render mark is dropped.
    expect(clearThreadRerenderStart).toHaveBeenCalledWith('t1');
  });

  it('clears the flag and shows the error toast on API failure, still without scrolling', async () => {
    apiAnswerThreadQuestion.mockRejectedValueOnce(new ApiError(500, 'boom'));
    const ok = await answerThreadQuestion('t1', 'tool-use-1', { kind: 'Selected', option_id: 'opt-a' });

    expect(ok).toBe(false);
    expect(scrollToBottom).not.toHaveBeenCalled();
    expect(clearThreadAnswering).toHaveBeenCalledWith('t1');
    expect(clearThreadRerenderStart).toHaveBeenCalledWith('t1');
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast.mock.calls[0][0]).toContain('boom');
  });
});
