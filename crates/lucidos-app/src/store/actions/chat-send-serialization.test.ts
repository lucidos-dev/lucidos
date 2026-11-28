import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.hoisted(() => {
  const storage = new Map<string, string>();
  (globalThis as any).localStorage = {
    getItem: (k: string) => storage.get(k) ?? null,
    setItem: (k: string, v: string) => storage.set(k, v),
    removeItem: (k: string) => storage.delete(k),
    clear: () => storage.clear(),
    get length() { return storage.size; },
    key: (_i: number) => null,
  };
  if (typeof globalThis.document === 'undefined') {
    (globalThis as any).document = {};
  }
  if (!(globalThis.document as any).querySelector) {
    (globalThis.document as any).querySelector = () => null;
  }
  if (!(globalThis.document as any).querySelectorAll) {
    (globalThis.document as any).querySelectorAll = () => [];
  }
  if (typeof globalThis.requestAnimationFrame === 'undefined') {
    (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
  }
  if (typeof globalThis.crypto === 'undefined' || !(globalThis.crypto as any).randomUUID) {
    (globalThis as any).crypto = {
      randomUUID: () => 'test-uuid-' + Math.random().toString(36).slice(2),
    };
  }
  if (typeof globalThis.window === 'undefined') {
    (globalThis as any).window = {};
  }
  if (typeof (globalThis as any).window.location === 'undefined') {
    (globalThis as any).window.location = { origin: 'https://localhost:5173' };
  }
});

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
  submitChat: vi.fn(),
  answerThreadQuestion: vi.fn(),
  cancelChat: vi.fn(),
  stopClaudeCode: vi.fn(),
  putComposeOnThread: vi.fn().mockResolvedValue({ status: 'applied' }),
  ensureThreadStarted: vi.fn().mockResolvedValue(undefined),
  deleteThread: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('./thread-navigation', () => ({
  pushThreadNavState: vi.fn(),
  removeThreadNavEntries: vi.fn(),
}));


vi.mock('./thread-loading', () => ({
  refreshThreadEvents: vi.fn().mockResolvedValue(true),
}));

vi.mock('./devices', () => ({
  getDeviceId: vi.fn(() => 'device-test'),
}));

vi.mock('../../utils/platform', () => ({
  isTauri: vi.fn(() => false),
  isIOS: () => false,
}));

vi.mock('../../utils/tauri', () => ({
  getWebviewContent: vi.fn(),
}));

import { focusedThreadId, threadMap, selectedScope, connectionStatus, panelOverlay } from '../store';
import { retryUnsentMessage, sendMessage } from './chat';
import { answerThreadQuestion } from './chat-claude-code';
import { SEND_RETRY_BACKOFF_MS } from './sendRetry';
import { pageOwnerId } from '../pageOwner';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { getDeviceId } from './devices';
import { makeOptimisticThreadState } from '../thread-events';
import { _resetComposeDraftsForTesting, getDraft, patchDraft } from '../composeDrafts';
import { ApiError, submitChat, answerThreadQuestion as apiAnswerThreadQuestion } from '../../api/client';
import { unsentMessages } from '../unsentMessages';
import { liveUnsentMessageRecords } from '../unsentMessageRecords';
import { acceptComposeReport, parseComposeReport, _resetComposeSyncForTesting } from '../composeSync';
import { isTauri } from '../../utils/platform';
import { getWebviewContent } from '../../utils/tauri';

const mockedSubmitChat = vi.mocked(submitChat);
const mockedIsTauri = vi.mocked(isTauri);
const mockedGetWebviewContent = vi.mocked(getWebviewContent);

const THREAD_A = '11111111-1111-4111-8111-111111111111';
const THREAD_B = '22222222-2222-4222-8222-222222222222';

function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void; reject: (e: unknown) => void } {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

function seedActiveThread(id: string): void {
  const next = new Map(threadMap.value);
  next.set(id, makeOptimisticThreadState({
    id,
    title: 'seeded',
    channel: 'chat',
    initiator: 'user',
    eventsLoaded: true,
    state: 'active',
    status: 'idle',
  }));
  threadMap.value = next;
}

async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

const sentMessages = () => mockedSubmitChat.mock.calls.map(c => (c[0] as { message: string }).message);

interface PositionedBody { message: string; event_id: string; input_chain: string; input_seq: number }
const sentBodies = () => mockedSubmitChat.mock.calls.map(c => c[0] as unknown as PositionedBody);

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
  selectedScope.value = { kind: 'lucidos' };
  connectionStatus.value = 'connected';
  panelOverlay.value = null;
  mockedSubmitChat.mockReset();
  mockedIsTauri.mockReset();
  mockedIsTauri.mockReturnValue(false);
  mockedGetWebviewContent.mockReset();
  _resetComposeDraftsForTesting();
  _resetComposeSyncForTesting();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('per-thread send serialization', () => {
  it('rebases rapid ordinary sends through their acknowledgements', async () => {
    seedActiveThread(THREAD_A);
    acceptComposeReport(THREAD_A, parseComposeReport({ summary_version: 7, compose_epoch: 7 }));
    const first = deferred<{ event_id: string; compose: { summary_version: number; compose_epoch: number } }>();
    mockedSubmitChat.mockReturnValueOnce(first.promise);
    for (const epoch of [8, 9]) mockedSubmitChat.mockImplementationOnce(async (body) => {
      expect(body.compose_epoch).toBe(epoch);
      return { event_id: body.event_id!, compose: { summary_version: epoch + 1, compose_epoch: epoch + 1 } };
    });
    const sends = ['first', 'second', 'third'].map(message => sendMessage(message, undefined, { threadId: THREAD_A, focus: false }));
    expect(mockedSubmitChat.mock.calls[0][0].compose_epoch).toBe(7);
    first.resolve({ event_id: mockedSubmitChat.mock.calls[0][0].event_id!, compose: { summary_version: 8, compose_epoch: 8 } });
    expect(await Promise.all(sends)).toEqual(['sent', 'sent', 'sent']);
  });

  it('retains a stale chained intent after saved acceptance and renews it only on explicit Retry', async () => {
    seedActiveThread(THREAD_A);
    acceptComposeReport(THREAD_A, parseComposeReport({ summary_version: 7, compose_epoch: 7 }));
    const first = deferred<{ event_id: string; compose: { summary_version: number; compose_epoch: number } }>();
    mockedSubmitChat.mockReturnValueOnce(first.promise).mockRejectedValueOnce(
      new ApiError(409, 'The saved draft changed before this send', { reason: 'compose_snapshot_stale' }),
    );
    const p1 = sendMessage('first', undefined, { threadId: THREAD_A, focus: false });
    const images = ['a'.repeat(64)];
    const p2 = sendMessage('keep stale content', images, { threadId: THREAD_A, focus: false });
    acceptComposeReport(THREAD_A, parseComposeReport({ summary_version: 9, compose_epoch: 9 }));
    patchDraft(THREAD_A, { text: 'newer local draft', image_hashes: [], mode: 'lucidos' });
    first.resolve({ event_id: mockedSubmitChat.mock.calls[0][0].event_id!, compose: { summary_version: 8, compose_epoch: 8 } });
    expect(await Promise.all([p1, p2])).toEqual(['sent', 'shown-as-failed']);
    const stale = mockedSubmitChat.mock.calls[1][0];
    expect(stale.compose_epoch).toBe(8);
    expect(getDraft(THREAD_A).text).toBe('newer local draft');
    expect(liveUnsentMessageRecords().find(record => record.eventId === stale.event_id)).toMatchObject({
      body: { message: 'keep stale content', image_hashes: images, compose_epoch: 8 },
      refusal: { reason: 'compose_snapshot_stale' },
    });
    mockedSubmitChat.mockResolvedValueOnce({ event_id: stale.event_id!, compose: { summary_version: 10, compose_epoch: 10 } });
    expect(await retryUnsentMessage(stale.event_id!)).toBe('sent');
    expect(mockedSubmitChat.mock.calls[2][0]).toMatchObject({ event_id: stale.event_id, compose_epoch: 9 });
  });

  it('retains a conflicting send and its images for an explicit retry', async () => {
    seedActiveThread(THREAD_A);
    const images = ['a'.repeat(64), 'b'.repeat(64)];
    mockedSubmitChat.mockRejectedValueOnce(new ApiError(409, 'A later input overtook this send', { reason: 'overtaken' }));
    const outcome = await sendMessage('keep this message', images, { threadId: THREAD_A, focus: false });
    const original = mockedSubmitChat.mock.calls[0][0];
    expect(outcome).toBe('shown-as-failed');
    expect(unsentMessages.value.get(original.event_id!)).toMatchObject({ body: original });
    expect(liveUnsentMessageRecords().find(r => r.eventId === original.event_id)).toMatchObject({
      phase: 'unsent', body: original, refusal: { reason: 'overtaken', message: 'A later input overtook this send' },
    });
    const newerDraft = { text: 'newer draft', image_hashes: ['c'.repeat(64)], mode: 'lucidos' as const };
    patchDraft(THREAD_A, newerDraft);
    mockedSubmitChat.mockRejectedValueOnce(new ApiError(409, 'Still overtaken', { reason: 'overtaken' }));
    expect(await retryUnsentMessage(original.event_id!)).toBe('shown-as-failed');
    expect(getDraft(THREAD_A)).toEqual(newerDraft);
    expect(unsentMessages.value.get(original.event_id!)?.body).toMatchObject({
      message: 'keep this message', image_hashes: images, event_id: original.event_id,
    });
    mockedSubmitChat.mockResolvedValueOnce({ event_id: original.event_id! });
    expect(await retryUnsentMessage(original.event_id!)).toBe('sent');
    expect(mockedSubmitChat.mock.calls[1][0]).toMatchObject({
      message: 'keep this message', image_hashes: images, event_id: original.event_id,
    });
    expect(unsentMessages.value.has(original.event_id!)).toBe(false);
  });

  it('holds the second POST until the first settles, and sends them in order', async () => {
    seedActiveThread(THREAD_A);
    const first = deferred<{ event_id: string }>();
    mockedSubmitChat
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce({ event_id: 'e2' });

    const p1 = sendMessage('first', undefined, { threadId: THREAD_A, focus: false });
    const p2 = sendMessage('second', undefined, { threadId: THREAD_A, focus: false });
    await flush();

    expect(sentMessages()).toEqual(['first']);

    first.resolve({ event_id: 'e1' });
    await Promise.all([p1, p2]);

    expect(sentMessages()).toEqual(['first', 'second']);
  });

  it('renders both optimistic rows immediately, in send order, while the first POST is open', async () => {
    seedActiveThread(THREAD_A);
    const first = deferred<{ event_id: string }>();
    mockedSubmitChat
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce({ event_id: 'e2' });

    const p1 = sendMessage('first', undefined, { threadId: THREAD_A, focus: false });
    const p2 = sendMessage('second', undefined, { threadId: THREAD_A, focus: false });
    await flush();

    const pending = threadMap.value.get(THREAD_A)!.pendingUserMessages;
    expect(pending.map(m => m.text)).toEqual(['first', 'second']);

    first.resolve({ event_id: 'e1' });
    await Promise.all([p1, p2]);
  });

  it('a refused send does not block the next one', async () => {
    seedActiveThread(THREAD_A);
    const first = deferred<{ event_id: string }>();
    mockedSubmitChat
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce({ event_id: 'e2' });

    const p1 = sendMessage('first', undefined, { threadId: THREAD_A, focus: false });
    const p2 = sendMessage('second', undefined, { threadId: THREAD_A, focus: false });
    await flush();

    first.reject(new ApiError(500, 'DB error'));
    await Promise.all([p1, p2]);

    expect(sentMessages()).toEqual(['first', 'second']);
  });

  it('a send that is still retrying holds every later send, so they reach the engine in order', async () => {
    vi.useFakeTimers();
    seedActiveThread(THREAD_A);
    mockedSubmitChat
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce({ event_id: 'e1' })
      .mockResolvedValue({ event_id: 'e' });

    const sends = ['A', 'B', 'C'].map(m => sendMessage(m, undefined, { threadId: THREAD_A, focus: false }));
    await vi.advanceTimersByTimeAsync(SEND_RETRY_BACKOFF_MS[0]);
    expect(sentMessages()).toEqual(['A', 'A']);
    await vi.advanceTimersByTimeAsync(SEND_RETRY_BACKOFF_MS[1]);
    await Promise.all(sends);

    expect(sentMessages()).toEqual(['A', 'A', 'A', 'B', 'C']);
  });

  it('a send that throws before its POST frees the slot at once', async () => {
    seedActiveThread(THREAD_A);
    mockedSubmitChat.mockResolvedValue({ event_id: 'e' });
    vi.mocked(getDeviceId).mockImplementationOnce(() => { throw new Error('boom'); });

    await expect(sendMessage('broken', undefined, { threadId: THREAD_A, focus: false })).rejects.toThrow('boom');
    expect(threadMap.value.get(THREAD_A)!.pendingUserMessages.map(m => m.text)).toEqual([]);
    const p = sendMessage('next', undefined, { threadId: THREAD_A, focus: false });
    await flush();
    expect(sentMessages()).toEqual(['next']);
    await p;
  });

  it('a send that throws while an earlier one retries still keeps later sends behind that one', async () => {
    vi.useFakeTimers();
    seedActiveThread(THREAD_A);
    mockedSubmitChat
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValue({ event_id: 'e' });

    const first = sendMessage('A', undefined, { threadId: THREAD_A, focus: false });
    await vi.advanceTimersByTimeAsync(0);
    vi.mocked(getDeviceId).mockImplementationOnce(() => { throw new Error('boom'); });
    await expect(sendMessage('broken', undefined, { threadId: THREAD_A, focus: false })).rejects.toThrow('boom');
    const later = sendMessage('C', undefined, { threadId: THREAD_A, focus: false });
    await vi.advanceTimersByTimeAsync(0);
    expect(sentMessages()).toEqual(['A']);

    await vi.advanceTimersByTimeAsync(SEND_RETRY_BACKOFF_MS[0]);
    await Promise.all([first, later]);
    expect(sentMessages()).toEqual(['A', 'A', 'C']);
  });

  it('a message sent after an answer to a question card waits for the answer', async () => {
    seedActiveThread(THREAD_A);
    const answer = deferred<boolean>();
    mockedSubmitChat.mockResolvedValue({ event_id: 'e' });

    acceptComposeReport(THREAD_A, parseComposeReport({ summary_version: 7, compose_epoch: 7 }));
    vi.mocked(apiAnswerThreadQuestion).mockImplementationOnce(async (_thread, _tool, _answer, _position, accepted) => {
      await answer.promise;
      accepted?.({ summary_version: 8, compose_epoch: 8, text: '', image_hashes: [] });
      return true;
    });
    const answering = answerThreadQuestion(THREAD_A, 'synthetic-question', { kind: 'FreeText', text: 'the answer' });
    const p = sendMessage('after the tap', undefined, { threadId: THREAD_A, focus: false });
    await flush();
    expect(sentMessages()).toEqual([]);

    answer.resolve(true);
    await Promise.all([answering, p]);
    expect(sentMessages()).toEqual(['after the tap']);
    expect(mockedSubmitChat.mock.calls[0][0].compose_epoch).toBe(8);
  });

  it('a successor waits for an unsettled send however long it takes, with no timer', async () => {
    vi.useFakeTimers();
    seedActiveThread(THREAD_A);
    const unsettled = deferred<{ event_id: string }>();
    mockedSubmitChat
      .mockReturnValueOnce(unsettled.promise)
      .mockResolvedValueOnce({ event_id: 'e2' });

    const p1 = sendMessage('first', undefined, { threadId: THREAD_A, focus: false });
    const p2 = sendMessage('second', undefined, { threadId: THREAD_A, focus: false });
    await vi.advanceTimersByTimeAsync(10 * 60_000);
    expect(sentMessages()).toEqual(['first']);

    unsettled.resolve({ event_id: 'e1' });
    await Promise.all([p1, p2]);
    expect(sentMessages()).toEqual(['first', 'second']);
  });

  it('the chain holds no timer', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    const source = readFileSync(resolve(here, 'sendChain.ts'), 'utf8');
    expect(source).not.toMatch(/setTimeout|setInterval/);
  });

  it('each send carries the page\'s input chain and a sequence in the order it was pressed', async () => {
    seedActiveThread(THREAD_A);
    seedActiveThread(THREAD_B);
    mockedSubmitChat.mockResolvedValue({ event_id: 'e' });

    await Promise.all([
      sendMessage('first', undefined, { threadId: THREAD_A, focus: false }),
      sendMessage('on B', undefined, { threadId: THREAD_B, focus: false }),
      sendMessage('second', undefined, { threadId: THREAD_A, focus: false }),
    ]);

    const [first, onB, second] = sentBodies();
    for (const body of [first, onB, second]) expect(body.input_chain).toBe(pageOwnerId);
    expect(second.input_seq).toBe(first.input_seq + 1);
    expect(onB.input_seq).not.toBe(first.input_seq + 1);
  });

  it('a quiet retry resends the same sequence, since it is the same input', async () => {
    vi.useFakeTimers();
    seedActiveThread(THREAD_A);
    mockedSubmitChat
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce({ event_id: 'e1' });

    const p = sendMessage('retried', undefined, { threadId: THREAD_A, focus: false });
    await vi.advanceTimersByTimeAsync(SEND_RETRY_BACKOFF_MS[0]);
    await p;

    const [attempt, retry] = sentBodies();
    expect(attempt.input_seq).toBeGreaterThan(0);
    expect(retry.input_seq).toBe(attempt.input_seq);
    expect(retry.event_id).toBe(attempt.event_id);
  });

  it('a manual Retry after an unknown outcome preserves the exact wire intent', async () => {
    vi.useFakeTimers();
    seedActiveThread(THREAD_A);
    mockedSubmitChat.mockRejectedValue(new TypeError('Load failed'));
    const unsent = sendMessage('not sent', undefined, { threadId: THREAD_A, focus: false });
    for (const delay of SEND_RETRY_BACKOFF_MS) await vi.advanceTimersByTimeAsync(delay);
    expect(await unsent).toBe('shown-as-failed');
    const original = sentBodies()[0];

    mockedSubmitChat.mockReset();
    mockedSubmitChat.mockResolvedValue({ event_id: 'e' });
    await sendMessage('sent after', undefined, { threadId: THREAD_A, focus: false });
    await retryUnsentMessage(original.event_id);

    const [after, retried] = sentBodies();
    expect(retried.event_id).toBe(original.event_id);
    expect(retried).toEqual(original);
    expect(retried.input_seq).toBeLessThan(after.input_seq);
  });

  it('does not serialize across different threads', async () => {
    seedActiveThread(THREAD_A);
    seedActiveThread(THREAD_B);
    const openOnA = deferred<{ event_id: string }>();
    mockedSubmitChat
      .mockReturnValueOnce(openOnA.promise)
      .mockResolvedValueOnce({ event_id: 'e2' });

    const p1 = sendMessage('on A', undefined, { threadId: THREAD_A, focus: false });
    const p2 = sendMessage('on B', undefined, { threadId: THREAD_B, focus: false });
    await flush();

    expect(sentMessages()).toEqual(['on A', 'on B']);

    openOnA.resolve({ event_id: 'e1' });
    await Promise.all([p1, p2]);
  });

  it('claims the chain slot at call time, so an await before the POST cannot reorder', async () => {
    seedActiveThread(THREAD_A);
    panelOverlay.value = { type: 'url-preview', url: 'https://example.com/page' };
    mockedIsTauri.mockReturnValue(true);
    const slowExtract = deferred<{ title: string; content: string }>();
    mockedGetWebviewContent
      .mockReturnValueOnce(slowExtract.promise)
      .mockResolvedValueOnce({ title: 'second', content: 'second body' });
    mockedSubmitChat.mockResolvedValue({ event_id: 'e' });

    const p1 = sendMessage('first', undefined, { threadId: THREAD_A, focus: false });
    const p2 = sendMessage('second', undefined, { threadId: THREAD_A, focus: false });

    await flush();
    expect(sentMessages()).toEqual([]);

    slowExtract.resolve({ title: 'first', content: 'first body' });
    await Promise.all([p1, p2]);

    expect(sentMessages()).toEqual(['first', 'second']);
  });

  it('keeps the URL it read when the preview closes during page extraction', async () => {
    seedActiveThread(THREAD_A);
    panelOverlay.value = { type: 'url-preview', url: 'https://example.com/page' };
    mockedIsTauri.mockReturnValue(true);
    const slowExtract = deferred<{ title: string; content: string }>();
    mockedGetWebviewContent.mockReturnValueOnce(slowExtract.promise);
    mockedSubmitChat.mockResolvedValue({ event_id: 'e' });

    const sent = sendMessage('about this page', undefined, { threadId: THREAD_A, focus: false });
    await flush();
    panelOverlay.value = null;
    slowExtract.resolve({ title: 'Page', content: 'body' });
    await sent;

    expect(mockedSubmitChat.mock.calls[0][0].url_context).toEqual({
      url: 'https://example.com/page', title: 'Page', content: 'body',
    });
  });

  it('dispatches a lone send synchronously, without deferring a microtask', async () => {
    seedActiveThread(THREAD_A);
    mockedSubmitChat.mockResolvedValue({ event_id: 'e' });

    const p = sendMessage('lone', undefined, { threadId: THREAD_A, focus: false });
    expect(sentMessages()).toEqual(['lone']);
    await p;
  });

  it('a settled chain is dropped, so a later lone send is not delayed', async () => {
    seedActiveThread(THREAD_A);
    mockedSubmitChat.mockResolvedValue({ event_id: 'e' });

    await sendMessage('first', undefined, { threadId: THREAD_A, focus: false });
    const p = sendMessage('second', undefined, { threadId: THREAD_A, focus: false });
    await flush();
    expect(sentMessages()).toEqual(['first', 'second']);
    await p;
  });
});
