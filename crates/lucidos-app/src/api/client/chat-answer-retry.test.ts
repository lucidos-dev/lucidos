/**
 * The answer POST is one attempt. The store action retries a stale
 * connection, as every send does (`withQuietRetries`). A retry here too would
 * multiply the attempts the user waits through.
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

describe('answerThreadQuestion', () => {
  it('posts the answer once', async () => {
    const mock = withFetch(() => Promise.resolve(new Response('{"ok":true}', { status: 200 })));

    await expect(answerThreadQuestion('t1', 'tool-1', answer)).resolves.toBe(true);
    expect(mock).toHaveBeenCalledTimes(1);
    expect(String(mock.mock.calls[0][0])).toContain('/threads/t1/answer-question');
  });

  it('gives up on a stalled attempt, so the store action can retry it', async () => {
    const mock = withFetch(() => Promise.resolve(new Response('{"ok":true}', { status: 200 })));

    await answerThreadQuestion('t1', 'tool-1', answer);
    expect(mock.mock.calls[0][1]?.signal).toBeInstanceOf(AbortSignal);
  });

  it('leaves a dropped connection to the store action to retry', async () => {
    const mock = withFetch(() => Promise.reject(new TypeError('Load failed')));

    await expect(answerThreadQuestion('t1', 'tool-1', answer)).rejects.toThrow('Load failed');
    expect(mock).toHaveBeenCalledTimes(1);
  });

  it('reports a 409 as false', async () => {
    // The question is already answered or gone.
    withFetch(() => Promise.resolve(new Response('{"error":"no pending question"}', { status: 409 })));

    await expect(answerThreadQuestion('t1', 'tool-1', answer)).resolves.toBe(false);
  });

  it('raises an ApiError carrying the engine reason on a 500', async () => {
    withFetch(() => Promise.resolve(new Response('{"error":"boom"}', { status: 500 })));

    await expect(answerThreadQuestion('t1', 'tool-1', answer)).rejects.toBeInstanceOf(ApiError);
  });
});
