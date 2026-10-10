/**
 * A typed answer whose POST gets no answer stays on the question card it
 * answers. It shows "Not sent" with a retry instead of an unsent row below the
 * card. It falls back to that row only when the card was answered some other
 * way, so the text is never lost. A new answer from this device replaces it.
 * Plan: docs/plans/2026-10-03-typed-answer-stays-on-its-card.md.
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
  if (typeof globalThis.document === 'undefined') (globalThis as any).document = {};
  if (!(globalThis.document as any).querySelector) (globalThis.document as any).querySelector = () => null;
  if (!(globalThis.document as any).querySelectorAll) (globalThis.document as any).querySelectorAll = () => [];
  if (typeof globalThis.requestAnimationFrame === 'undefined') {
    (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
  }
});

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
  submitChat: vi.fn(),
}));
vi.mock('./thread-navigation', () => ({ pushThreadNavState: vi.fn(), removeThreadNavEntries: vi.fn() }));
vi.mock('../../components/chat/scrollState', () => ({ followSentMessage: vi.fn(), stopFollowingBottom: vi.fn() }));
vi.mock('./thread-loading', () => ({
  refreshThreadEvents: vi.fn().mockResolvedValue(true),
  forgetThreadEventsFailures: vi.fn(),
}));
vi.mock('./devices', () => ({ getDeviceId: () => 'device-test' }));
vi.mock('../../utils/platform', () => ({ isTauri: () => false, isIOS: () => false }));

import { focusedThreadId, threadMap } from '../store';
import { retryUnsentMessage, sendMessage, showRestoredUnsentMessage } from './chat';
import { unsentMessages } from '../unsentMessages';
import { _resetUnsentMessageRecordsForTesting } from '../unsentMessageRecords';
import { submitChat } from '../../api/client';
import { computeExchanges, handleEvent, makeOptimisticThreadState, type StoredEvent } from '../thread-events';

const mockedSubmitChat = vi.mocked(submitChat);

/** A send that stays in flight until the test lets it go. No unsettled POST
 *  then carries into the next test through the thread's send chain. */
function holdNextSend(): () => void {
  let release!: () => void;
  mockedSubmitChat.mockImplementationOnce(() => new Promise((resolve) => {
    release = () => resolve({ event_id: 'held' } as never);
  }));
  return () => release();
}
const TS = '2026-10-03T10:00:00.000Z';

/** Thread `t` with question card `tu-1` open, focused. */
function threadWaitingOnQuestion() {
  const thread = makeOptimisticThreadState({ id: 't', title: 'Q', channel: 'chat', initiator: 'user', eventsLoaded: true });
  thread.meta = { ...thread.meta, status: 'waiting_for_user_answer' };
  threadMap.value = new Map([['t', thread]]);
  handleEvent(threadMap.value, 't', 1, { type: 'MessageReceived', text: 'go', channel: 'chat' } as StoredEvent, TS, 'e-go');
  handleEvent(threadMap.value, 't', 2, {
    type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: 's', question: 'Which?', options: [{ id: 'a', label: 'A' }],
  } as StoredEvent, TS);
  focusedThreadId.value = 't';
}

const exchanges = () => computeExchanges(threadMap.value.get('t')!);
const card = () => exchanges().find(ex => ex.userEvent.type === 'UserQuestionAsked')!;

/** A typed answer none of whose attempts gets an answer, quiet retries included. */
async function answerUnanswered(text: string): Promise<string> {
  mockedSubmitChat.mockRejectedValue(new TypeError('Load failed'));
  const outcome = sendMessage(text, undefined, { threadId: 't' });
  await vi.runAllTimersAsync();
  expect(await outcome).toBe('shown-as-failed');
  mockedSubmitChat.mockReset();
  const [eventId] = [...unsentMessages.value.keys()];
  return eventId;
}

beforeEach(() => {
  vi.useFakeTimers();
  unsentMessages.value = new Map();
  mockedSubmitChat.mockReset();
  _resetUnsentMessageRecordsForTesting();
  threadWaitingOnQuestion();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('a typed answer that gets no answer', () => {
  it('stays on its card as not sent, with no row of its own', async () => {
    const eventId = await answerUnanswered('neither, do C');
    expect(exchanges()).toHaveLength(2);
    expect(card().typedAnswer).toEqual({ state: 'unsent', text: 'neither, do C', image_hashes: [], unsentEventId: eventId });
    expect(unsentMessages.value.get(eventId)?.answersQuestion).toBe('tu-1');
  });

  it('goes back to sending on its card when retried', async () => {
    const eventId = await answerUnanswered('neither, do C');
    const release = holdNextSend();
    const retried = retryUnsentMessage(eventId);
    expect(exchanges()).toHaveLength(2);
    expect(card().typedAnswer).toEqual({ state: 'sending', text: 'neither, do C', image_hashes: [] });
    release();
    await retried;
  });

  it('is replaced by a new typed answer from this device', async () => {
    await answerUnanswered('neither, do C');
    const release = holdNextSend();
    const sent = sendMessage('C after all', undefined, { threadId: 't' });
    expect(unsentMessages.value.size).toBe(0);
    expect(exchanges()).toHaveLength(2);
    expect(card().typedAnswer).toEqual({ state: 'sending', text: 'C after all', image_hashes: [] });
    release();
    await sent;
  });

  it('falls back to its row when the card was answered some other way', async () => {
    await answerUnanswered('neither, do C');
    handleEvent(threadMap.value, 't', 3, {
      type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'a' },
    } as StoredEvent, TS);
    const all = exchanges();
    expect(card().typedAnswer).toBeUndefined();
    expect(all.some(ex => ex.userEvent.type === 'MessageReceived' && (ex.userEvent as { text?: string }).text === 'neither, do C')).toBe(true);
  });

  it('falls back to its row, with Discard, once the card stops waiting', async () => {
    await answerUnanswered('neither, do C');
    const thread = threadMap.value.get('t')!;
    thread.meta = { ...thread.meta, status: 'idle' };
    expect(card().typedAnswer).toBeUndefined();
    expect(exchanges().some(ex => ex.userEvent.type === 'MessageReceived' && (ex.userEvent as { text?: string }).text === 'neither, do C')).toBe(true);
  });

  it('counts as sent when the engine recorded it as the card\'s answer before the POST was lost', async () => {
    mockedSubmitChat.mockImplementationOnce(async (body) => {
      handleEvent(threadMap.value, 't', 3, {
        type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'FreeText', text: body.message },
      } as StoredEvent, TS);
      throw new TypeError('Load failed');
    });
    expect(await sendMessage('neither, do C', undefined, { threadId: 't' })).toBe('sent');
    expect(unsentMessages.value.size).toBe(0);
  });

  it('returns to its card after a reload', () => {
    showRestoredUnsentMessage({
      eventId: 'e-restored',
      threadId: 't',
      body: { message: 'from before the reload', mode: 'human', event_id: 'e-restored', thread_id: 't' } as never,
      settlement: { kind: 'follow-up' },
      sentAt: TS,
      phase: 'unsent',
      failedRetries: 0,
      ownerId: 'old-page',
      answersQuestion: 'tu-1',
    });
    expect(exchanges()).toHaveLength(2);
    expect(card().typedAnswer).toMatchObject({ state: 'unsent', text: 'from before the reload', unsentEventId: 'e-restored' });
  });
});
