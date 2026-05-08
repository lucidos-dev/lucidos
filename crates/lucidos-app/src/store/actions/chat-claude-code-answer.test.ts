import { describe, it, expect, beforeEach, vi } from 'vitest';

const showToast = vi.fn();
const apiAnswerCCQuestion = vi.fn(async () => true);
const scrollToBottom = vi.fn();

class ApiError extends Error {
  constructor(public readonly httpCode: number, public readonly reason: string) {
    super(`${httpCode} ${reason}`);
  }
}

vi.mock('../store', () => ({ showToast }));
vi.mock('../../api/client', () => ({
  answerCCQuestion: apiAnswerCCQuestion,
  ApiError,
}));
vi.mock('../../components/chat/scrollState', () => ({ scrollToBottom }));
vi.mock('./thread-sync', () => ({}));
vi.mock('./threads', () => ({}));

const { answerCCQuestion } = await import('./chat-claude-code');

describe('answerCCQuestion', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiAnswerCCQuestion.mockResolvedValue(true);
  });

  it('pins to bottom before sending so the answered card height-shrink does not unstick scroll', async () => {
    await answerCCQuestion('t1', 'tool-use-1', { kind: 'Selected', option_id: 'opt-a' });

    expect(scrollToBottom).toHaveBeenCalledTimes(1);
    const scrollOrder = scrollToBottom.mock.invocationCallOrder[0];
    const apiOrder = apiAnswerCCQuestion.mock.invocationCallOrder[0];
    expect(scrollOrder).toBeLessThan(apiOrder);
  });

  it('still pins to bottom on API failure so the error toast is visible at the bottom', async () => {
    apiAnswerCCQuestion.mockRejectedValueOnce(new ApiError(500, 'boom'));
    const ok = await answerCCQuestion('t1', 'tool-use-1', { kind: 'Selected', option_id: 'opt-a' });

    expect(ok).toBe(false);
    expect(scrollToBottom).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast.mock.calls[0][0]).toContain('boom');
  });
});
