import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

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
  // Inlined rather than re-exported: the real one lives in `_core`, which pulls
  // in the store. The same three browser wordings as `isTransportError`.
  isTransientFetchError: (err: unknown) =>
    err instanceof TypeError && /Load failed|Failed to fetch|NetworkError/i.test(err.message),
}));
vi.mock('../../utils/clientLog', () => ({ postClientLog: vi.fn() }));
vi.mock('../../components/chat/scrollState', () => ({ scrollToBottom }));
vi.mock('../../utils/threadOpenMarks', () => ({ markThreadRerenderStart, clearThreadRerenderStart }));
vi.mock('../../utils/renderPhaseTimers', () => ({ currentPerfBaseline: () => ({ start: 0, md: 0, link: 0 }) }));
vi.mock('./thread-sync', () => ({}));
vi.mock('./threads', () => ({}));
vi.mock('./sendChain', () => ({ inSendChain: (_threadId: string, send: () => Promise<unknown>) => send() }));

const { answerThreadQuestion, answerFailureMessage, retryUnsentPick } = await import('./chat-claude-code');
const { pendingAnswers, unsentPicks } = await import('../pendingDecisions');
const { SEND_RETRY_BACKOFF_MS } = await import('./sendRetry');

const ALL_RETRIES_MS = SEND_RETRY_BACKOFF_MS.reduce((a, b) => a + b, 0);
const ATTEMPTS = SEND_RETRY_BACKOFF_MS.length + 1;
const pick = { kind: 'Selected', option_id: 'opt-a' } as const;

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
    const outcome = await answerThreadQuestion('t1', 'tool-use-1', { kind: 'Selected', option_id: 'opt-a' });

    expect(outcome).toBe('refused');
    expect(markThreadAnswering).toHaveBeenCalledWith('t1');
    expect(clearThreadAnswering).toHaveBeenCalledWith('t1');
    // Perf: no resume render is coming → the stale re-render mark is dropped.
    expect(clearThreadRerenderStart).toHaveBeenCalledWith('t1');
  });

  it('clears the flag and shows the error toast on API failure, still without scrolling', async () => {
    apiAnswerThreadQuestion.mockRejectedValueOnce(new ApiError(500, 'boom'));
    const outcome = await answerThreadQuestion('t1', 'tool-use-1', { kind: 'Selected', option_id: 'opt-a' });

    expect(outcome).toBe('refused');
    expect(scrollToBottom).not.toHaveBeenCalled();
    expect(clearThreadAnswering).toHaveBeenCalledWith('t1');
    expect(clearThreadRerenderStart).toHaveBeenCalledWith('t1');
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast.mock.calls[0][0]).toContain('boom');
  });
});

/**
 * One failed tap, one message, and it names the cause.
 *
 * The reported pair was "Could not send answer. Please try again." over
 * "Failed to send answer: unknown error", twice each for two taps. The action
 * raised one and the submit site raised the other, so the count is the
 * contract: the callers roll their optimistic state back and stay quiet.
 */
describe('the one failure message', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiAnswerThreadQuestion.mockResolvedValue(true);
  });

  it('is raised on a 409 too, which used to be the caller-only case', async () => {
    apiAnswerThreadQuestion.mockResolvedValueOnce(false);
    await answerThreadQuestion('t1', 'tool-use-1', { kind: 'Selected', option_id: 'opt-a' });

    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast.mock.calls[0][0]).toContain('no longer waiting');
  });


  it('does not ask for a retry that would conflict forever', () => {
    expect(answerFailureMessage({ kind: 'conflict' })).not.toMatch(/try again/i);
  });

  it('keeps the engine reason for a real HTTP verdict', () => {
    expect(answerFailureMessage({ kind: 'error', err: new ApiError(500, 'boom') }))
      .toBe('Could not send answer: boom');
  });

  it('falls back to the error detail for anything else', () => {
    expect(answerFailureMessage({ kind: 'error', err: new Error('kaboom') }))
      .toContain('kaboom');
  });
});

describe('a tapped answer that gets no answer', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
    unsentPicks.clear('tool-use-1');
    pendingAnswers.clear('tool-use-1');
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  async function answerSettled(): Promise<string> {
    const outcome = answerThreadQuestion('t1', 'tool-use-1', pick);
    await vi.advanceTimersByTimeAsync(ALL_RETRIES_MS + 1);
    return outcome;
  }

  it('retries quietly and is sent when a retry gets through, with no toast', async () => {
    apiAnswerThreadQuestion
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce(true);
    expect(await answerSettled()).toBe('sent');
    expect(apiAnswerThreadQuestion).toHaveBeenCalledTimes(2);
    expect(showToast).not.toHaveBeenCalled();
    expect(clearThreadAnswering).not.toHaveBeenCalled();
  });

  it('stays answering while it retries', async () => {
    apiAnswerThreadQuestion.mockRejectedValue(new TypeError('Load failed'));
    const outcome = answerThreadQuestion('t1', 'tool-use-1', pick);
    await vi.advanceTimersByTimeAsync(SEND_RETRY_BACKOFF_MS[0]);
    expect(clearThreadAnswering).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(ALL_RETRIES_MS);
    await outcome;
  });

  it('counts a 409 on a retry as landed: the first attempt got there', async () => {
    apiAnswerThreadQuestion
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce(false);
    expect(await answerSettled()).toBe('sent');
    expect(showToast).not.toHaveBeenCalled();
  });

  it('keeps the pick as unsent after the last attempt, with no toast', async () => {
    apiAnswerThreadQuestion.mockRejectedValue(new TypeError('Load failed'));
    expect(await answerSettled()).toBe('unsent');
    expect(apiAnswerThreadQuestion).toHaveBeenCalledTimes(ATTEMPTS);
    expect(showToast).not.toHaveBeenCalled();
    expect(clearThreadAnswering).toHaveBeenCalledWith('t1');
    expect(unsentPicks.map.value.get('tool-use-1')).toEqual({
      threadId: 't1',
      answer: pick,
      failure: { attempts: ATTEMPTS, reason: 'Load failed' },
    });
  });

  it('Retry sends the same answer again, sending on the card meanwhile', async () => {
    apiAnswerThreadQuestion.mockRejectedValue(new TypeError('Load failed'));
    await answerSettled();
    apiAnswerThreadQuestion.mockReset();
    let answer!: (ok: boolean) => void;
    apiAnswerThreadQuestion.mockReturnValueOnce(new Promise<boolean>(resolve => { answer = resolve; }));

    const retried = retryUnsentPick('tool-use-1');
    expect(pendingAnswers.map.value.get('tool-use-1')).toEqual(pick);
    expect(unsentPicks.map.value.has('tool-use-1')).toBe(false);
    expect(apiAnswerThreadQuestion).toHaveBeenCalledWith('t1', 'tool-use-1', pick);
    answer(true);
    expect(await retried).toBe('sent');
  });

  it('a new answer to the card replaces the unsent pick', async () => {
    apiAnswerThreadQuestion.mockRejectedValue(new TypeError('Load failed'));
    await answerSettled();
    apiAnswerThreadQuestion.mockReset();
    apiAnswerThreadQuestion.mockResolvedValueOnce(true);
    await answerThreadQuestion('t1', 'tool-use-1', { kind: 'Selected', option_id: 'opt-b' });
    expect(unsentPicks.map.value.has('tool-use-1')).toBe(false);
  });

  it('Retry on a pick that is no longer unsent does nothing', async () => {
    expect(await retryUnsentPick('tool-use-1')).toBeNull();
    expect(apiAnswerThreadQuestion).not.toHaveBeenCalled();
  });
});
