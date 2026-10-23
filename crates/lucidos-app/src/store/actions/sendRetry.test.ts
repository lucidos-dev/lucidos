/**
 * The quiet retries every send path shares: which failures retry, when, how
 * many times, and what ends a wait early.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('../../utils/clientLog', () => ({ postClientLog: vi.fn() }));

import { ApiError } from '../../api/client';
import { postClientLog } from '../../utils/clientLog';
import { SEND_RETRY_BACKOFF_MS, SEND_RETRY_DEADLINE_MS, withQuietRetries } from './sendRetry';
import { _resetPageVisitForTesting } from '../../utils/pageVisit';

const dropped = () => new TypeError('Load failed');

/** A window and document the early wake can listen on. */
function stubPage() {
  const win = new EventTarget();
  const doc = Object.assign(new EventTarget(), { visibilityState: 'visible' });
  vi.stubGlobal('window', win);
  vi.stubGlobal('document', doc);
  return { win, doc };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(postClientLog).mockClear();
});

afterEach(() => {
  _resetPageVisitForTesting();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('withQuietRetries', () => {
  it('runs the first attempt synchronously, so a lone send still goes out at once', () => {
    const attempt = vi.fn().mockResolvedValue('ok');
    void withQuietRetries(attempt, { path: 'message' });
    expect(attempt).toHaveBeenCalledTimes(1);
  });

  it('a first success returns its value and logs nothing', async () => {
    const result = await withQuietRetries(async () => 'ok', { path: 'message' });
    expect(result).toEqual({ kind: 'done', value: 'ok', attempts: 1 });
    expect(postClientLog).not.toHaveBeenCalled();
  });

  it.each([
    ['a dropped connection', dropped()],
    ['a browser cancel', new DOMException('x', 'AbortError')],
    ['a timeout', new DOMException('x', 'TimeoutError')],
    ['the boot splash', new ApiError(503, 'Lucidos is restarting', undefined, true)],
  ])('retries %s three times, after 1, 3 and 8 seconds, then gives up', async (_name, error) => {
    const attempt = vi.fn().mockRejectedValue(error);
    const settled = vi.fn();
    void withQuietRetries(attempt, { path: 'message' }).then(settled);

    await vi.advanceTimersByTimeAsync(0);
    expect(attempt).toHaveBeenCalledTimes(1);
    for (const [i, delay] of SEND_RETRY_BACKOFF_MS.entries()) {
      await vi.advanceTimersByTimeAsync(delay - 1);
      expect(attempt).toHaveBeenCalledTimes(i + 1);
      await vi.advanceTimersByTimeAsync(1);
      expect(attempt).toHaveBeenCalledTimes(i + 2);
    }
    await vi.advanceTimersByTimeAsync(0);
    expect(settled).toHaveBeenCalledWith({ kind: 'gave-up', error, attempts: 4 });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(attempt).toHaveBeenCalledTimes(4);
  });

  it('a success on a retry is done, and the retry is logged', async () => {
    const attempt = vi.fn()
      .mockRejectedValueOnce(dropped())
      .mockRejectedValueOnce(dropped())
      .mockResolvedValueOnce('ok');
    const result = withQuietRetries(attempt, { path: 'answer' });
    await vi.advanceTimersByTimeAsync(SEND_RETRY_BACKOFF_MS[0] + SEND_RETRY_BACKOFF_MS[1]);
    expect(await result).toEqual({ kind: 'done', value: 'ok', attempts: 3 });
    expect(postClientLog).toHaveBeenCalledWith('chat', 'send_retried', {
      path: 'answer', attempts: 3, outcome: 'done', error: 'TypeError',
    });
  });

  it('a verdict is refused at once, with no retry', async () => {
    const verdict = new ApiError(409, 'thread is locked');
    const attempt = vi.fn().mockRejectedValue(verdict);
    expect(await withQuietRetries(attempt, { path: 'message' })).toEqual({ kind: 'refused', error: verdict, attempts: 1 });
    expect(attempt).toHaveBeenCalledTimes(1);
  });

  it('a verdict on a retry is refused there', async () => {
    const verdict = new ApiError(409, 'thread is locked');
    const attempt = vi.fn().mockRejectedValueOnce(dropped()).mockRejectedValueOnce(verdict);
    const result = withQuietRetries(attempt, { path: 'message' });
    await vi.advanceTimersByTimeAsync(SEND_RETRY_BACKOFF_MS[0]);
    expect(await result).toEqual({ kind: 'refused', error: verdict, attempts: 2 });
  });

  it('starts no attempt past the deadline, so a stalled send retries once', async () => {
    const attempt = vi.fn().mockImplementation(() => new Promise((_resolve, reject) => {
      setTimeout(() => reject(new DOMException('x', 'TimeoutError')), 20_000);
    }));
    const settled = vi.fn();
    void withQuietRetries(attempt, { path: 'message' }).then(settled);
    await vi.advanceTimersByTimeAsync(20_000 + SEND_RETRY_BACKOFF_MS[0] + 20_000);
    expect(attempt).toHaveBeenCalledTimes(2);
    expect(settled).toHaveBeenCalledWith(expect.objectContaining({ kind: 'gave-up', attempts: 2 }));
    expect(SEND_RETRY_DEADLINE_MS).toBeLessThan(20_000 + SEND_RETRY_BACKOFF_MS[0] + 20_000 + SEND_RETRY_BACKOFF_MS[1]);
  });

  it('gives up when a backoff timer fires past the deadline', async () => {
    const attempt = vi.fn().mockRejectedValue(dropped());
    const settled = vi.fn();
    void withQuietRetries(attempt, { path: 'message' }).then(settled);
    await vi.advanceTimersByTimeAsync(0);
    // The page froze through the wait, so the clock jumps past the deadline.
    vi.setSystemTime(Date.now() + SEND_RETRY_DEADLINE_MS + 1);
    await vi.advanceTimersByTimeAsync(SEND_RETRY_BACKOFF_MS[0]);
    expect(attempt).toHaveBeenCalledTimes(1);
    expect(settled).toHaveBeenCalledWith(expect.objectContaining({ kind: 'gave-up', attempts: 1 }));
  });

  it('stops when the send turns out to have landed', async () => {
    let landed = false;
    const attempt = vi.fn().mockRejectedValue(dropped());
    const result = withQuietRetries(attempt, { path: 'message', landed: () => landed });
    await vi.advanceTimersByTimeAsync(0);
    landed = true;
    await vi.advanceTimersByTimeAsync(SEND_RETRY_BACKOFF_MS[0]);
    expect(await result).toEqual({ kind: 'landed', attempts: 1 });
    expect(attempt).toHaveBeenCalledTimes(1);
  });

  it('asks the caller which failures retry, with how long the attempt took', async () => {
    const attempt = vi.fn().mockRejectedValue(new ApiError(502, 'model failed'));
    const retryable = vi.fn((err: unknown) => err instanceof ApiError && err.httpCode >= 500);
    const result = withQuietRetries(attempt, { path: 'side-question', retryable });
    await vi.advanceTimersByTimeAsync(SEND_RETRY_BACKOFF_MS[0]);
    expect(retryable).toHaveBeenCalledWith(expect.any(ApiError), expect.any(Number));
    expect(attempt).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(60_000);
    expect((await result).kind).toBe('gave-up');
  });

  it('tells the caller before each retry', async () => {
    const beforeRetry = vi.fn();
    const attempt = vi.fn().mockRejectedValueOnce(dropped()).mockResolvedValueOnce('ok');
    const result = withQuietRetries(attempt, { path: 'message', beforeRetry });
    await vi.advanceTimersByTimeAsync(SEND_RETRY_BACKOFF_MS[0]);
    await result;
    expect(beforeRetry).toHaveBeenCalledTimes(1);
    expect(beforeRetry).toHaveBeenCalledWith(expect.any(TypeError));
  });

  it('retries at once when the browser reports the network back', async () => {
    const { win } = stubPage();
    const attempt = vi.fn().mockRejectedValueOnce(dropped()).mockResolvedValueOnce('ok');
    const result = withQuietRetries(attempt, { path: 'message' });
    await vi.advanceTimersByTimeAsync(0);
    win.dispatchEvent(new Event('online'));
    await vi.advanceTimersByTimeAsync(0);
    expect(attempt).toHaveBeenCalledTimes(2);
    expect((await result).kind).toBe('done');
  });

  it('retries at once when the page becomes visible again, not when it hides', async () => {
    const { doc } = stubPage();
    const attempt = vi.fn().mockRejectedValueOnce(new DOMException('x', 'AbortError')).mockResolvedValueOnce('ok');
    void withQuietRetries(attempt, { path: 'message' });
    await vi.advanceTimersByTimeAsync(0);
    doc.visibilityState = 'hidden';
    doc.dispatchEvent(new Event('visibilitychange'));
    await vi.advanceTimersByTimeAsync(0);
    expect(attempt).toHaveBeenCalledTimes(1);
    doc.visibilityState = 'visible';
    doc.dispatchEvent(new Event('visibilitychange'));
    await vi.advanceTimersByTimeAsync(0);
    expect(attempt).toHaveBeenCalledTimes(2);
  });
});
