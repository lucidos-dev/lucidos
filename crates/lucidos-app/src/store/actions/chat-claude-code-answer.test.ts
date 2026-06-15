import { describe, it, expect, beforeEach, vi } from 'vitest';

const showToast = vi.fn();
const apiAnswerThreadQuestion = vi.fn(async () => true);
const scrollToBottom = vi.fn();
const markThreadAnswering = vi.fn();
const clearThreadAnswering = vi.fn();

class ApiError extends Error {
  constructor(public readonly httpCode: number, public readonly reason: string) {
    super(`${httpCode} ${reason}`);
  }
}

vi.mock('../store', () => ({ showToast, markThreadAnswering, clearThreadAnswering }));
vi.mock('../../api/client', () => ({
  answerThreadQuestion: apiAnswerThreadQuestion,
  ApiError,
}));
vi.mock('../../components/chat/scrollState', () => ({ scrollToBottom }));
vi.mock('./thread-sync', () => ({}));
vi.mock('./threads', () => ({}));

const { answerThreadQuestion } = await import('./chat-claude-code');

describe('answerThreadQuestion', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiAnswerThreadQuestion.mockResolvedValue(true);
  });

  it('pins to bottom before sending so the answered card height-shrink does not unstick scroll', async () => {
    await answerThreadQuestion('t1', 'tool-use-1', { kind: 'Selected', option_id: 'opt-a' });

    expect(scrollToBottom).toHaveBeenCalledTimes(1);
    const scrollOrder = scrollToBottom.mock.invocationCallOrder[0];
    const apiOrder = apiAnswerThreadQuestion.mock.invocationCallOrder[0];
    expect(scrollOrder).toBeLessThan(apiOrder);
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
  });

  it('clears the optimistic answering flag on a 409 (stale/duplicate, no resume coming)', async () => {
    apiAnswerThreadQuestion.mockResolvedValueOnce(false);
    const ok = await answerThreadQuestion('t1', 'tool-use-1', { kind: 'Selected', option_id: 'opt-a' });

    expect(ok).toBe(false);
    expect(markThreadAnswering).toHaveBeenCalledWith('t1');
    expect(clearThreadAnswering).toHaveBeenCalledWith('t1');
  });

  it('still pins to bottom on API failure, clears the flag, and shows the error toast at the bottom', async () => {
    apiAnswerThreadQuestion.mockRejectedValueOnce(new ApiError(500, 'boom'));
    const ok = await answerThreadQuestion('t1', 'tool-use-1', { kind: 'Selected', option_id: 'opt-a' });

    expect(ok).toBe(false);
    expect(scrollToBottom).toHaveBeenCalledTimes(1);
    expect(clearThreadAnswering).toHaveBeenCalledWith('t1');
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast.mock.calls[0][0]).toContain('boom');
  });
});
