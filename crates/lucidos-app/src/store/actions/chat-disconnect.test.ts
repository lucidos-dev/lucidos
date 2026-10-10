/**
 * Verifies what a send does when its POST gets no answer (transport error):
 * the text stays in the thread as an unsent message, and Retry re-posts the
 * same request. HTTP refusals on a first send are covered by
 * chat-orphan-thread.test.ts. Stale `connectionStatus` must NOT short-circuit a
 * send that would have succeeded.
 */
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
});

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
  submitChat: vi.fn(),
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

vi.mock('../../components/chat/scrollState', () => ({
  followSentMessage: vi.fn(),
  stopFollowingBottom: vi.fn(),
}));

vi.mock('./thread-loading', () => ({
  refreshThreadEvents: vi.fn().mockResolvedValue(true),
  forgetThreadEventsFailures: vi.fn(),
}));

vi.mock('./devices', () => ({
  getDeviceId: () => 'device-test',
}));

vi.mock('../../utils/platform', () => ({
  isTauri: () => false,
  isIOS: () => false,
}));

import {
  focusedThreadId,
  threadMap,
  selectedScope,
  connectionStatus,
} from '../store';
import { discardUnsentMessage, sendMessage, retryUnsentMessage, unsentMessageCopy } from './chat';
import { SEND_RETRY_BACKOFF_MS } from './sendRetry';
import { readUnsentMessageStore, _resetUnsentMessageRecordsForTesting } from '../unsentMessageRecords';
import { _resetComposeDraftsForTesting, getDraft } from '../composeDrafts';
import { forgetUnsentMessagesOfThread, unsentMessages } from '../unsentMessages';
import { settleDeliveredUnsentMessage } from './sendSettlement';
import { getComposeSelectionOverride, patchComposeSelection, _resetComposeSelectionsForTesting } from '../composeSelections';
import { ApiError, submitChat } from '../../api/client';
import { groupIntoExchanges, isUnsentExchange, exchangeError, exchangeUserMessage, exchangeUserImageHashes, handleEvent, makeOptimisticThreadState, type StoredEvent } from '../thread-events';

const mockedSubmitChat = vi.mocked(submitChat);

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
  selectedScope.value = { kind: 'lucidos' };
  connectionStatus.value = 'connected';
  unsentMessages.value = new Map();
  mockedSubmitChat.mockReset();
  _resetComposeDraftsForTesting();
  _resetComposeSelectionsForTesting();
  _resetUnsentMessageRecordsForTesting();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

const ALL_RETRIES_MS = SEND_RETRY_BACKOFF_MS.reduce((a, b) => a + b, 0);
/** POSTs a send makes when none gets an answer. */
const ATTEMPTS = SEND_RETRY_BACKOFF_MS.length + 1;

/** Run a send through its quiet retries. Stops short of the accepted send's
 *  safety sweep, which would clear the pending row a test asserts on. */
async function settle<T>(p: Promise<T>): Promise<T> {
  await vi.advanceTimersByTimeAsync(ALL_RETRIES_MS + 1);
  return p;
}

const send = (...args: Parameters<typeof sendMessage>) => settle(sendMessage(...args));
const retry = (eventId: string) => settle(retryUnsentMessage(eventId));

/** Every attempt of the next send, its quiet retries included, gets no answer. */
function rejectEveryAttempt(error: unknown): void {
  for (let i = 0; i <= SEND_RETRY_BACKOFF_MS.length; i++) mockedSubmitChat.mockRejectedValueOnce(error);
}

function exchangesOf(threadId: string) {
  return groupIntoExchanges(threadMap.value.get(threadId)!.events);
}

/** A compose draft's first message that got no answer, on thread `t-1` with a
 *  model pick. Returns the unsent event id. */
async function sendFirstUnanswered(text: string): Promise<string> {
  threadMap.value = new Map([['t-1', makeOptimisticThreadState({ id: 't-1', title: '', channel: 'chat', initiator: 'user', eventsLoaded: true })]]);
  patchComposeSelection('t-1', { model: 'claude-sonnet-5' });
  rejectEveryAttempt(new TypeError('Load failed'));
  await send(text, undefined, { threadId: 't-1', settlement: { kind: 'first-send', mode: 'lucidos' } });
  const [eventId] = [...unsentMessages.value.keys()];
  return eventId;
}

/** Send once with no answer, and return the thread and the unsent event id. */
async function sendUnanswered(text: string, imageHashes?: string[]) {
  rejectEveryAttempt(new TypeError('Load failed'));
  const outcome = await send(text, imageHashes);
  expect(outcome).toBe('shown-as-failed');
  const threadId = focusedThreadId.value!;
  const [eventId] = [...unsentMessages.value.keys()];
  return { threadId, eventId };
}

describe('a send that gets no answer', () => {
  it.each(['Failed to fetch', 'Load failed', 'NetworkError when attempting to fetch resource.'])(
    'shows the text as an unsent message with its own copy (%s)',
    async (message) => {
      rejectEveryAttempt(new TypeError(message));
      await send('important question');

      const threadId = focusedThreadId.value!;
      const exchanges = exchangesOf(threadId);
      expect(exchanges).toHaveLength(1);
      expect(exchangeUserMessage(exchanges[0])).toBe('important question');
      expect(exchangeError(exchanges[0])?.message).toBe(unsentMessageCopy(0));
      expect(isUnsentExchange(exchanges[0])).toBe(true);
      expect(unsentMessages.value.size).toBe(1);
    },
  );

  it.each(['TimeoutError', 'AbortError'])(
    'a POST that timed out or that the browser cancelled is unsent too, never dropped (%s)',
    async (name) => {
      rejectEveryAttempt(new DOMException('no answer', name));
      expect(await send('still here')).toBe('shown-as-failed');
      expect(exchangeUserMessage(exchangesOf(focusedThreadId.value!)[0])).toBe('still here');
      expect(unsentMessages.value.size).toBe(1);
    },
  );

  it('retries quietly, keeping the message sending, and shows Not sent only after the last attempt', async () => {
    rejectEveryAttempt(new TypeError('Load failed'));
    const outcome = sendMessage('patience');
    const threadId = focusedThreadId.value!;
    for (const delay of SEND_RETRY_BACKOFF_MS) {
      await vi.advanceTimersByTimeAsync(delay - 1);
      expect(threadMap.value.get(threadId)!.pendingUserMessages.map(p => p.text)).toEqual(['patience']);
      expect(unsentMessages.value.size).toBe(0);
      await vi.advanceTimersByTimeAsync(1);
    }
    expect(await outcome).toBe('shown-as-failed');
    expect(mockedSubmitChat).toHaveBeenCalledTimes(ATTEMPTS);
    expect(unsentMessages.value.size).toBe(1);
  });

  it('keeps what the retries ended on, for the Not sent details', async () => {
    rejectEveryAttempt(new TypeError('Load failed'));
    await send('details please');
    const [unsent] = [...unsentMessages.value.values()];
    expect(unsent.failure).toEqual({ attempts: ATTEMPTS, reason: 'Load failed' });
  });

  it('a send that gets through on a retry is sent, with no card', async () => {
    mockedSubmitChat
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce({ event_id: 'e' });
    expect(await send('third time lucky')).toBe('sent');
    const threadId = focusedThreadId.value!;
    expect(exchangesOf(threadId).filter(ex => exchangeError(ex))).toEqual([]);
    expect(threadMap.value.get(threadId)!.pendingUserMessages.map(p => p.text)).toEqual(['third time lucky']);
    expect(unsentMessages.value.size).toBe(0);
    expect(await readUnsentMessageStore()).toEqual([]);
  });

  it('a verdict after a dropped attempt is shown at once, with no further retry', async () => {
    mockedSubmitChat
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockRejectedValueOnce(new ApiError(400, 'bad request'));
    expect(await send('refused on retry')).toBe('dropped');
    expect(mockedSubmitChat).toHaveBeenCalledTimes(2);
    expect(unsentMessages.value.size).toBe(0);
  });

  it('a send whose own row arrives during a backoff is sent, with no repeat POST', async () => {
    mockedSubmitChat.mockResolvedValueOnce({ event_id: 'first' });
    await send('opens the thread');
    const threadId = focusedThreadId.value!;
    rejectEveryAttempt(new TypeError('Load failed'));
    const outcome = sendMessage('arrives late');
    await vi.advanceTimersByTimeAsync(0);
    const body = mockedSubmitChat.mock.calls[1][0];
    handleEvent(threadMap.value, threadId, 88, {
      type: 'MessageReceived',
      text: body.message,
    } as StoredEvent, new Date().toISOString(), body.event_id);

    expect(await settle(outcome)).toBe('sent');
    expect(mockedSubmitChat).toHaveBeenCalledTimes(2);
  });

  it('blames neither the network nor a disconnect', () => {
    for (const copy of [unsentMessageCopy(0), unsentMessageCopy(1)]) {
      expect(copy.toLowerCase()).not.toMatch(/disconnected|connection|reload/);
      expect(copy.toLowerCase()).toMatch(/did not answer/);
    }
  });

  it('keeps the images on the unsent message', async () => {
    const { threadId } = await sendUnanswered('with a picture', ['hash-a']);
    expect(exchangeUserImageHashes(exchangesOf(threadId)[0])).toEqual(['hash-a']);
  });

  it('a send whose own row arrived before the lost answer counts as sent', async () => {
    mockedSubmitChat.mockResolvedValueOnce({ event_id: 'first' });
    await send('opens the thread');
    const threadId = focusedThreadId.value!;
    mockedSubmitChat.mockImplementationOnce(async (body) => {
      handleEvent(threadMap.value, threadId, 77, {
        type: 'MessageReceived',
        text: body.message,
      } as StoredEvent, new Date().toISOString(), body.event_id);
      throw new TypeError('Load failed');
    });

    expect(await send('it landed')).toBe('sent');

    expect(unsentMessages.value.size).toBe(0);
    const failed = exchangesOf(threadId).filter(ex => exchangeError(ex));
    expect(failed).toEqual([]);
  });

  it('stale disconnected status does not stop a send that goes through', async () => {
    connectionStatus.value = 'disconnected';
    mockedSubmitChat.mockResolvedValueOnce({ event_id: 'srv-evt' });

    await send('this should go through');

    expect(mockedSubmitChat).toHaveBeenCalledTimes(1);
    for (const ex of exchangesOf(focusedThreadId.value!)) {
      expect(exchangeError(ex)).toBeNull();
    }
    expect(unsentMessages.value.size).toBe(0);
  });
});

describe('Retry on an unsent message', () => {
  it('re-posts the exact first request, event id included', async () => {
    const { eventId } = await sendUnanswered('try me again');
    mockedSubmitChat.mockResolvedValueOnce({ event_id: eventId });

    await retry(eventId);

    expect(mockedSubmitChat).toHaveBeenCalledTimes(ATTEMPTS + 1);
    expect(mockedSubmitChat.mock.calls[ATTEMPTS][0]).toEqual(mockedSubmitChat.mock.calls[0][0]);
    expect(mockedSubmitChat.mock.calls[ATTEMPTS][0].event_id).toBe(eventId);
  });

  it('an accepted retry leaves one pending message and no failure card', async () => {
    const { threadId, eventId } = await sendUnanswered('try me again');
    mockedSubmitChat.mockResolvedValueOnce({ event_id: eventId });

    expect(await retry(eventId)).toBe('sent');

    const thread = threadMap.value.get(threadId)!;
    expect([...thread.events.keys()].filter(seq => seq < 0)).toEqual([]);
    expect(thread.pendingUserMessages.map(p => p.eventId)).toEqual([eventId]);
    expect(unsentMessages.value.size).toBe(0);
  });

  it('consumes a first send\'s picks only once a retry is accepted', async () => {
    const eventId = await sendFirstUnanswered('compose first send');
    expect(getComposeSelectionOverride('t-1').model).toBe('claude-sonnet-5');

    rejectEveryAttempt(new TypeError('Load failed'));
    await retry(eventId);
    expect(getComposeSelectionOverride('t-1').model).toBe('claude-sonnet-5');

    mockedSubmitChat.mockResolvedValueOnce({ event_id: eventId });
    await retry(eventId);
    expect(getComposeSelectionOverride('t-1').model).toBeUndefined();
  });

  it('a retry that also gets no answer shows the unsent message again', async () => {
    const { threadId, eventId } = await sendUnanswered('still nothing');
    rejectEveryAttempt(new TypeError('Load failed'));

    expect(await retry(eventId)).toBe('shown-as-failed');

    const exchanges = exchangesOf(threadId);
    expect(exchanges).toHaveLength(1);
    expect(exchangeUserMessage(exchanges[0])).toBe('still nothing');
    expect(exchangeError(exchanges[0])?.message).toBe(unsentMessageCopy(1));
    expect(unsentMessages.value.get(eventId)?.failedRetries).toBe(1);
  });

  it('a refused retry puts the text back in the composer', async () => {
    mockedSubmitChat.mockResolvedValueOnce({ event_id: 'first' });
    await send('opens the thread');
    const threadId = focusedThreadId.value!;
    rejectEveryAttempt(new TypeError('Load failed'));
    await send('a follow-up');
    const [eventId] = [...unsentMessages.value.keys()];
    mockedSubmitChat.mockRejectedValueOnce(new ApiError(409, 'thread is locked'));

    expect(await retry(eventId)).toBe('dropped');

    expect(getDraft(threadId).text).toBe('a follow-up');
    expect(threadMap.value.get(threadId)!.pendingUserMessages.some(p => p.eventId === eventId)).toBe(false);
  });

  it('a refused retry of a first send rolls its draft back, mode and picks included', async () => {
    const eventId = await sendFirstUnanswered('compose first send');
    mockedSubmitChat.mockRejectedValueOnce(new ApiError(409, 'locked'));

    await retry(eventId);

    expect(threadMap.value.get('t-1')!.meta.state).toBe('composing');
    expect(getDraft('t-1').text).toBe('compose first send');
    expect(getDraft('t-1').mode).toBe('lucidos');
    expect(getComposeSelectionOverride('t-1').model).toBe('claude-sonnet-5');
  });

  it('a refused retry of a new thread starts a fresh draft with its text and images', async () => {
    const { threadId, eventId } = await sendUnanswered('a brand-new thread', ['hash-a']);
    mockedSubmitChat.mockRejectedValueOnce(new ApiError(400, 'bad request'));

    expect(await retry(eventId)).toBe('dropped');

    expect(threadMap.value.has(threadId)).toBe(false);
    const draftId = focusedThreadId.value!;
    expect(draftId).not.toBe(threadId);
    expect(getDraft(draftId).text).toBe('a brand-new thread');
    expect(getDraft(draftId).image_hashes).toEqual(['hash-a']);
  });

  it('two unsent messages on one thread never share a row', async () => {
    mockedSubmitChat.mockResolvedValueOnce({ event_id: 'first' });
    await send('opens the thread');
    const threadId = focusedThreadId.value!;
    rejectEveryAttempt(new TypeError('Load failed'));
    await send('first unsent');
    rejectEveryAttempt(new TypeError('Load failed'));
    await send('second unsent');
    const [firstId, secondId] = [...unsentMessages.value.keys()];
    mockedSubmitChat.mockResolvedValueOnce({ event_id: secondId });

    await retry(secondId);

    const remaining = exchangesOf(threadId).filter(ex => exchangeError(ex));
    expect(remaining.map(exchangeUserMessage)).toEqual(['first unsent']);
    expect(unsentMessages.value.has(firstId)).toBe(true);
  });

  it('a second press while the first retry runs sends nothing', async () => {
    const { eventId } = await sendUnanswered('double tap');
    let answer!: (v: { event_id: string }) => void;
    mockedSubmitChat.mockReturnValueOnce(new Promise(resolve => { answer = resolve; }));

    const first = retryUnsentMessage(eventId);
    expect(await retry(eventId)).toBeNull();
    answer({ event_id: eventId });
    await first;

    expect(mockedSubmitChat).toHaveBeenCalledTimes(ATTEMPTS + 1);
  });

  it('the engine\'s own row for the message replaces the unsent pair and settles it', async () => {
    const eventId = await sendFirstUnanswered('it did land');
    const threadId = 't-1';

    const handled = handleEvent(threadMap.value, threadId, 42, {
      type: 'MessageReceived',
      text: 'it did land',
    } as StoredEvent, new Date().toISOString(), eventId);
    expect(handled.retiredUnsentEventId).toBe(eventId);
    settleDeliveredUnsentMessage(eventId);

    const exchanges = exchangesOf(threadId);
    expect(exchanges).toHaveLength(1);
    expect(exchangeError(exchanges[0])).toBeNull();
    expect([...threadMap.value.get(threadId)!.events.keys()]).toEqual([42]);
    expect(getComposeSelectionOverride(threadId).model).toBeUndefined();
    expect(unsentMessages.value.size).toBe(0);
  });
});

describe('the stored copy of a send', () => {
  const stored = async () => (await readUnsentMessageStore()).map(r => ({ eventId: r.eventId, phase: r.phase, failedRetries: r.failedRetries }));

  it('is kept from before the POST goes out, with the exact request', async () => {
    let answer!: (v: { event_id: string }) => void;
    mockedSubmitChat.mockReturnValueOnce(new Promise(resolve => { answer = resolve; }));

    const send = sendMessage('in flight', ['hash-a']);
    const [record] = await readUnsentMessageStore();

    expect(record.phase).toBe('sending');
    expect(record.body).toEqual(mockedSubmitChat.mock.calls[0][0]);
    expect(record.settlement).toEqual({ kind: 'raw-new' });
    expect(Number.isNaN(Date.parse(record.sentAt))).toBe(false);
    answer({ event_id: record.eventId });
    await send;
  });

  it('ends when the engine takes the send', async () => {
    mockedSubmitChat.mockResolvedValueOnce({ event_id: 'e' });
    await send('accepted');
    expect(await stored()).toEqual([]);
  });

  it('ends when the engine refuses the send', async () => {
    mockedSubmitChat.mockRejectedValueOnce(new ApiError(400, 'bad request'));
    await send('refused');
    expect(await stored()).toEqual([]);
  });

  it('stays as unsent when the send gets no answer, counting retries', async () => {
    const { eventId } = await sendUnanswered('no answer');
    expect(await stored()).toEqual([{ eventId, phase: 'unsent', failedRetries: 0 }]);

    rejectEveryAttempt(new TypeError('Load failed'));
    await retry(eventId);
    expect(await stored()).toEqual([{ eventId, phase: 'unsent', failedRetries: 1 }]);
  });

  it('ends when a retry is accepted, or refused', async () => {
    const first = await sendUnanswered('accept me');
    mockedSubmitChat.mockResolvedValueOnce({ event_id: first.eventId });
    await retry(first.eventId);
    expect(await stored()).toEqual([]);

    const second = await sendUnanswered('refuse me');
    mockedSubmitChat.mockRejectedValueOnce(new ApiError(409, 'locked'));
    await retry(second.eventId);
    expect(await stored()).toEqual([]);
  });

  it('ends when the engine\'s own row for it arrives', async () => {
    const eventId = await sendFirstUnanswered('it did land');
    handleEvent(threadMap.value, 't-1', 42, { type: 'MessageReceived', text: 'it did land' } as StoredEvent, new Date().toISOString(), eventId);
    settleDeliveredUnsentMessage(eventId);
    expect(await stored()).toEqual([]);
  });

  it('ends with its card when the thread is deleted', async () => {
    const { threadId, eventId } = await sendUnanswered('deleted with its thread');
    forgetUnsentMessagesOfThread(threadId);
    expect(unsentMessages.value.has(eventId)).toBe(false);
    expect(await stored()).toEqual([]);
  });
});

describe('Discard on an unsent message', () => {
  it('drops a follow-up\'s card and its stored copy, and sends nothing', async () => {
    mockedSubmitChat.mockResolvedValueOnce({ event_id: 'first' });
    await send('opens the thread');
    const threadId = focusedThreadId.value!;
    rejectEveryAttempt(new TypeError('Load failed'));
    await send('never mind');
    const [eventId] = [...unsentMessages.value.keys()];

    discardUnsentMessage(eventId);

    expect(exchangesOf(threadId).map(exchangeUserMessage)).not.toContain('never mind');
    expect(unsentMessages.value.size).toBe(0);
    expect(await readUnsentMessageStore()).toEqual([]);
    expect(mockedSubmitChat).toHaveBeenCalledTimes(1 + ATTEMPTS);
    expect(getDraft(threadId).text).toBe('');
  });

  it('removes the thread a raw new send would have made, which only this device has', async () => {
    const { threadId, eventId } = await sendUnanswered('a thread never made');

    discardUnsentMessage(eventId);

    expect(threadMap.value.has(threadId)).toBe(false);
    expect(focusedThreadId.value).toBeNull();
    expect(await readUnsentMessageStore()).toEqual([]);
  });

  it('leaves a first send\'s thread an empty draft that keeps its picks', async () => {
    const eventId = await sendFirstUnanswered('never mind');
    discardUnsentMessage(eventId);
    expect(threadMap.value.get('t-1')!.meta.state).toBe('composing');
    expect(getDraft('t-1').text).toBe('');
    expect(getComposeSelectionOverride('t-1').model).toBe('claude-sonnet-5');
  });
});
