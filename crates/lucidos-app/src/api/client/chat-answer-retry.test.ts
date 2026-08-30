/**
 * Answering a question survives a stale connection.
 *
 * Reported from an iOS PWA: a question card was tapped twice and both taps
 * failed with "Failed to send answer: unknown error". No answer reached the
 * engine either time, and the third tap a minute later landed. That is the
 * half-closed HTTP/2 connection a backgrounded PWA wakes up holding, which
 * WebKit rejects as `TypeError("Load failed")` before the request goes out.
 *
 * The neighbouring mutations (`stopClaudeCode`, `cancelChat`, the compose PUT)
 * already retry through `mutatingFetchIdempotent`. The answer POST did not, and
 * it is the one tap the agent is blocked on.
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import { answerThreadQuestion } from './chat';
import { ApiError } from './_core';

const originalFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = originalFetch;
  vi.restoreAllMocks();
});

function withFetch(...impls: Array<() => Promise<Response>>): ReturnType<typeof vi.fn> {
  const mock = vi.fn();
  for (const impl of impls) mock.mockImplementationOnce(impl);
  globalThis.fetch = mock as unknown as typeof fetch;
  return mock;
}

const answer = { kind: 'Selected', option_id: 'opt-0' } as const;

describe('answerThreadQuestion over a stale connection', () => {
  it('retries the POST once when the first attempt never leaves the device', async () => {
    const mock = withFetch(
      () => Promise.reject(new TypeError('Load failed')),
      () => Promise.resolve(new Response('{"ok":true}', { status: 200 })),
    );

    await expect(answerThreadQuestion('t1', 'tool-1', answer)).resolves.toBe(true);
    expect(mock).toHaveBeenCalledTimes(2);
    // The retry carries the same answer: a re-picked selection is exactly what
    // the user should not have to do.
    expect(mock.mock.calls[1][1]?.body).toBe(mock.mock.calls[0][1]?.body);
    expect(String(mock.mock.calls[1][0])).toContain('/threads/t1/answer-question');
  });

  it('surfaces a second transport failure rather than looping', async () => {
    const mock = withFetch(
      () => Promise.reject(new TypeError('Load failed')),
      () => Promise.reject(new TypeError('Load failed')),
    );

    await expect(answerThreadQuestion('t1', 'tool-1', answer)).rejects.toThrow('Load failed');
    expect(mock).toHaveBeenCalledTimes(2);
  });

  it('does not retry a rejection that says something about the request', async () => {
    const mock = withFetch(() => Promise.reject(new TypeError('answer is not JSON')));

    await expect(answerThreadQuestion('t1', 'tool-1', answer)).rejects.toThrow('not JSON');
    expect(mock).toHaveBeenCalledTimes(1);
  });

  it('reports a 409 as false without a second attempt', async () => {
    // The question is already answered or gone. Re-sending would 409 forever.
    const mock = withFetch(() =>
      Promise.resolve(new Response('{"error":"no pending question"}', { status: 409 })),
    );

    await expect(answerThreadQuestion('t1', 'tool-1', answer)).resolves.toBe(false);
    expect(mock).toHaveBeenCalledTimes(1);
  });

  it('raises an ApiError carrying the engine reason on a 500', async () => {
    withFetch(() => Promise.resolve(new Response('{"error":"boom"}', { status: 500 })));

    await expect(answerThreadQuestion('t1', 'tool-1', answer)).rejects.toBeInstanceOf(ApiError);
  });
});
