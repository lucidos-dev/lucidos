/**
 * Verifies what a send does when its POST gets no answer (transport error):
 * the text stays in the thread as an unsent message, and Retry re-posts the
 * same request. HTTP refusals on a first send are covered by
 * chat-orphan-thread.test.ts. Stale `connectionStatus` must NOT short-circuit a
 * send that would have succeeded.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

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
import { sendMessage, retryUnsentMessage, unsentMessageCopy } from './chat';
import { _resetComposeDraftsForTesting, getDraft } from '../composeDrafts';
import { settleDeliveredUnsentMessage, unsentMessages } from '../unsentMessages';
import { ApiError, submitChat } from '../../api/client';
import { groupIntoExchanges, exchangeError, exchangeUserMessage, exchangeUserImageHashes, handleEvent, type StoredEvent } from '../thread-events';

const mockedSubmitChat = vi.mocked(submitChat);

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
  selectedScope.value = { kind: 'lucidos' };
  connectionStatus.value = 'connected';
  unsentMessages.value = new Map();
  mockedSubmitChat.mockReset();
  _resetComposeDraftsForTesting();
});

function exchangesOf(threadId: string) {
  return groupIntoExchanges(threadMap.value.get(threadId)!.events);
}

/** Send once with no answer, and return the thread and the unsent event id. */
async function sendUnanswered(text: string, imageHashes?: string[]) {
  mockedSubmitChat.mockRejectedValueOnce(new TypeError('Load failed'));
  const outcome = await sendMessage(text, imageHashes);
  expect(outcome).toBe('shown-as-failed');
  const threadId = focusedThreadId.value!;
  const [eventId] = [...unsentMessages.value.keys()];
  return { threadId, eventId };
}

describe('a send that gets no answer', () => {
  it.each(['Failed to fetch', 'Load failed', 'NetworkError when attempting to fetch resource.'])(
    'shows the text as an unsent message with its own copy (%s)',
    async (message) => {
      mockedSubmitChat.mockRejectedValueOnce(new TypeError(message));
      await sendMessage('important question');

      const threadId = focusedThreadId.value!;
      const exchanges = exchangesOf(threadId);
      expect(exchanges).toHaveLength(1);
      expect(exchangeUserMessage(exchanges[0])).toBe('important question');
      expect(exchangeError(exchanges[0])?.message).toBe(unsentMessageCopy(0));
      expect(unsentMessages.value.size).toBe(1);
    },
  );

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
    await sendMessage('opens the thread');
    const threadId = focusedThreadId.value!;
    mockedSubmitChat.mockImplementationOnce(async (body) => {
      handleEvent(threadMap.value, threadId, 77, {
        type: 'MessageReceived',
        text: body.message,
      } as StoredEvent, new Date().toISOString(), body.event_id);
      throw new TypeError('Load failed');
    });

    expect(await sendMessage('it landed')).toBe('sent');

    expect(unsentMessages.value.size).toBe(0);
    const failed = exchangesOf(threadId).filter(ex => exchangeError(ex));
    expect(failed).toEqual([]);
  });

  it('stale disconnected status does not stop a send that goes through', async () => {
    connectionStatus.value = 'disconnected';
    mockedSubmitChat.mockResolvedValueOnce({ event_id: 'srv-evt' });

    await sendMessage('this should go through');

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

    await retryUnsentMessage(eventId);

    expect(mockedSubmitChat).toHaveBeenCalledTimes(2);
    expect(mockedSubmitChat.mock.calls[1][0]).toEqual(mockedSubmitChat.mock.calls[0][0]);
    expect(mockedSubmitChat.mock.calls[1][0].event_id).toBe(eventId);
  });

  it('an accepted retry leaves one pending message and no failure card', async () => {
    const { threadId, eventId } = await sendUnanswered('try me again');
    mockedSubmitChat.mockResolvedValueOnce({ event_id: eventId });

    expect(await retryUnsentMessage(eventId)).toBe('sent');

    const thread = threadMap.value.get(threadId)!;
    expect([...thread.events.keys()].filter(seq => seq < 0)).toEqual([]);
    expect(thread.pendingUserMessages.map(p => p.eventId)).toEqual([eventId]);
    expect(unsentMessages.value.size).toBe(0);
  });

  it('runs the sender\'s owed settle only once a retry is accepted', async () => {
    const onAccepted = vi.fn();
    mockedSubmitChat.mockRejectedValueOnce(new TypeError('Load failed'));
    await sendMessage('compose first send', undefined, { settlement: { onAccepted } });
    const [eventId] = [...unsentMessages.value.keys()];
    expect(onAccepted).not.toHaveBeenCalled();

    mockedSubmitChat.mockRejectedValueOnce(new TypeError('Load failed'));
    await retryUnsentMessage(eventId);
    expect(onAccepted).not.toHaveBeenCalled();

    mockedSubmitChat.mockResolvedValueOnce({ event_id: eventId });
    await retryUnsentMessage(eventId);
    expect(onAccepted).toHaveBeenCalledTimes(1);
  });

  it('a retry that also gets no answer shows the unsent message again', async () => {
    const { threadId, eventId } = await sendUnanswered('still nothing');
    mockedSubmitChat.mockRejectedValueOnce(new TypeError('Load failed'));

    expect(await retryUnsentMessage(eventId)).toBe('shown-as-failed');

    const exchanges = exchangesOf(threadId);
    expect(exchanges).toHaveLength(1);
    expect(exchangeUserMessage(exchanges[0])).toBe('still nothing');
    expect(exchangeError(exchanges[0])?.message).toBe(unsentMessageCopy(1));
    expect(unsentMessages.value.get(eventId)?.failedRetries).toBe(1);
  });

  it('a refused retry puts the text back in the composer', async () => {
    mockedSubmitChat.mockResolvedValueOnce({ event_id: 'first' });
    await sendMessage('opens the thread');
    const threadId = focusedThreadId.value!;
    mockedSubmitChat.mockRejectedValueOnce(new TypeError('Load failed'));
    await sendMessage('a follow-up');
    const [eventId] = [...unsentMessages.value.keys()];
    mockedSubmitChat.mockRejectedValueOnce(new ApiError(409, 'thread is locked'));

    expect(await retryUnsentMessage(eventId)).toBe('dropped');

    expect(getDraft(threadId).text).toBe('a follow-up');
    expect(threadMap.value.get(threadId)!.pendingUserMessages.some(p => p.eventId === eventId)).toBe(false);
  });

  it('a refused retry hands the text to the sender\'s own restore when it has one', async () => {
    const onRefused = vi.fn();
    mockedSubmitChat.mockRejectedValueOnce(new TypeError('Load failed'));
    await sendMessage('compose first send', undefined, { settlement: { onRefused } });
    const [eventId] = [...unsentMessages.value.keys()];
    mockedSubmitChat.mockRejectedValueOnce(new ApiError(409, 'locked'));

    await retryUnsentMessage(eventId);

    expect(onRefused).toHaveBeenCalledTimes(1);
  });

  it('a refused retry of a new thread starts a fresh draft with its text and images', async () => {
    const { threadId, eventId } = await sendUnanswered('a brand-new thread', ['hash-a']);
    mockedSubmitChat.mockRejectedValueOnce(new ApiError(400, 'bad request'));

    expect(await retryUnsentMessage(eventId)).toBe('dropped');

    expect(threadMap.value.has(threadId)).toBe(false);
    const draftId = focusedThreadId.value!;
    expect(draftId).not.toBe(threadId);
    expect(getDraft(draftId).text).toBe('a brand-new thread');
    expect(getDraft(draftId).image_hashes).toEqual(['hash-a']);
  });

  it('two unsent messages on one thread never share a row', async () => {
    mockedSubmitChat.mockResolvedValueOnce({ event_id: 'first' });
    await sendMessage('opens the thread');
    const threadId = focusedThreadId.value!;
    mockedSubmitChat.mockRejectedValueOnce(new TypeError('Load failed'));
    await sendMessage('first unsent');
    mockedSubmitChat.mockRejectedValueOnce(new TypeError('Load failed'));
    await sendMessage('second unsent');
    const [firstId, secondId] = [...unsentMessages.value.keys()];
    mockedSubmitChat.mockResolvedValueOnce({ event_id: secondId });

    await retryUnsentMessage(secondId);

    const remaining = exchangesOf(threadId).filter(ex => exchangeError(ex));
    expect(remaining.map(exchangeUserMessage)).toEqual(['first unsent']);
    expect(unsentMessages.value.has(firstId)).toBe(true);
  });

  it('a second press while the first retry runs sends nothing', async () => {
    const { eventId } = await sendUnanswered('double tap');
    let answer!: (v: { event_id: string }) => void;
    mockedSubmitChat.mockReturnValueOnce(new Promise(resolve => { answer = resolve; }));

    const first = retryUnsentMessage(eventId);
    expect(await retryUnsentMessage(eventId)).toBeNull();
    answer({ event_id: eventId });
    await first;

    expect(mockedSubmitChat).toHaveBeenCalledTimes(2);
  });

  it('the engine\'s own row for the message replaces the unsent pair and settles it', async () => {
    const onAccepted = vi.fn();
    mockedSubmitChat.mockRejectedValueOnce(new TypeError('Load failed'));
    await sendMessage('it did land', undefined, { settlement: { onAccepted } });
    const threadId = focusedThreadId.value!;
    const [eventId] = [...unsentMessages.value.keys()];

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
    expect(onAccepted).toHaveBeenCalledTimes(1);
    expect(unsentMessages.value.size).toBe(0);
  });
});
