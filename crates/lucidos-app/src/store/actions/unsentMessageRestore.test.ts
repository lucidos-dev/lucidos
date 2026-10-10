import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('./thread-loading', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./thread-loading')>();
  return { ...actual, ensureThreadByIdInMap: vi.fn() };
});

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  submitChat: vi.fn(),
  putComposeOnThread: vi.fn().mockResolvedValue({ status: 'applied' }),
  ensureThreadStarted: vi.fn().mockResolvedValue(undefined),
}));

import { installUnsentMessageRestore, restoreUnsentMessages, _resetUnsentMessageRestoreForTesting } from './unsentMessageRestore';
import { ensureThreadByIdInMap, upsertThread } from './thread-loading';
import type { ThreadSummary } from '../../api/threads';
import { forgetComposeState, noteServerDraft, _resetUndeliveredComposeDraftsForTesting } from './compose';
import { retryUnsentMessage, UNSENT_AFTER_RELOAD_COPY, unsentMessageCopy } from './chat';
import { settleDeliveredUnsentMessage } from './sendSettlement';
import { ApiError, submitChat } from '../../api/client';
import {
  createMemoryUnsentMessageBackend,
  readUnsentMessageStore,
  _resetUnsentMessageRecordsForTesting,
  type UnsentMessageBackend,
  type UnsentMessageRecord,
} from '../unsentMessageRecords';
import { unsentMessages } from '../unsentMessages';
import { PAGE_OWNER_LOCK_PREFIX, pageOwnerId } from '../pageOwner';
import { connectionStatus, focusedThreadId, threadListFetched, threadMap, toasts } from '../store';
import { getDraft, setDraft, _resetComposeDraftsForTesting } from '../composeDrafts';
import { getComposeSelectionOverride, patchComposeSelection, _resetComposeSelectionsForTesting } from '../composeSelections';
import {
  exchangeError,
  exchangeUserMessage,
  groupIntoExchanges,
  handleEvent,
  makeOptimisticThreadState,
  type StoredEvent,
  type ThreadState,
} from '../thread-events';

const LIVE = 't-live';
const EARLIER_PAGE = 'an-earlier-page';
const mockedSubmitChat = vi.mocked(submitChat);

function record(eventId: string, overrides: Partial<UnsentMessageRecord> = {}): UnsentMessageRecord {
  const threadId = overrides.threadId ?? LIVE;
  return {
    eventId,
    threadId,
    body: { message: `message ${eventId}`, mode: 'human', event_id: eventId, thread_id: threadId },
    settlement: { kind: 'follow-up' },
    sentAt: '2026-10-03T08:00:01.000Z',
    phase: 'unsent',
    failedRetries: 0,
    ownerId: EARLIER_PAGE,
    ...overrides,
  };
}

function thread(id: string, state: 'composing' | 'active' | 'discarded' = 'active'): ThreadState {
  return makeOptimisticThreadState({ id, title: '', channel: 'chat', initiator: 'user', eventsLoaded: true, state, status: 'idle' });
}

let backend: UnsentMessageBackend;

async function seed(...records: UnsentMessageRecord[]): Promise<void> {
  for (const r of records) await backend.put(r);
}

const storedIds = async () => (await readUnsentMessageStore()).map((r) => r.eventId).sort();
const warnings = () => toasts.value.filter((t) => t.type === 'warning').map((t) => t.message);
const exchangesOf = (threadId: string) => groupIntoExchanges(threadMap.value.get(threadId)!.events);

function engineRow(threadId: string, seq: number, text: string, created: string, eventId?: string): void {
  handleEvent(threadMap.value, threadId, seq, { type: 'MessageReceived', text } as StoredEvent, created, eventId);
}

beforeEach(() => {
  backend = createMemoryUnsentMessageBackend();
  _resetUnsentMessageRecordsForTesting(backend);
  _resetUnsentMessageRestoreForTesting();
  _resetComposeSelectionsForTesting();
  unsentMessages.value = new Map();
  toasts.value = [];
  connectionStatus.value = 'connected';
  focusedThreadId.value = null;
  threadMap.value = new Map([[LIVE, thread(LIVE)]]);
  vi.mocked(ensureThreadByIdInMap).mockReset().mockImplementation(async (id) => threadMap.value.has(id));
  mockedSubmitChat.mockReset();
});

afterEach(() => {
  // A refused Retry schedules a compose write; drop it with the thread.
  for (const id of threadMap.value.keys()) forgetComposeState(id);
  _resetUndeliveredComposeDraftsForTesting();
  _resetComposeDraftsForTesting();
  threadMap.value = new Map();
  threadListFetched.value = false;
  connectionStatus.value = 'disconnected';
  Reflect.deleteProperty(globalThis.navigator as object, 'locks');
});

describe('restoreUnsentMessages', () => {
  it('brings a message back as a Not sent card with Retry, owned by this page', async () => {
    await seed(record('e-1'));
    const outcome = await restoreUnsentMessages();

    expect(outcome.shown).toBe(1);
    const [exchange] = exchangesOf(LIVE);
    expect(exchangeUserMessage(exchange)).toBe('message e-1');
    expect(exchangeError(exchange)?.message).toBe(unsentMessageCopy(0));
    expect(unsentMessages.value.has('e-1')).toBe(true);
    expect((await readUnsentMessageStore())[0].ownerId).toBe(pageOwnerId);
    expect(warnings()).toEqual([]);
  });

  it('says a send cut off by the reload may not have been sent', async () => {
    await seed(record('e-1', { phase: 'sending' }), record('e-2', { failedRetries: 1, sentAt: '2026-10-03T08:00:02.000Z' }));
    await restoreUnsentMessages();
    const copies = exchangesOf(LIVE).map((ex) => exchangeError(ex)?.message);
    expect(copies).toEqual([UNSENT_AFTER_RELOAD_COPY, unsentMessageCopy(1)]);
  });

  it('a Retry after the reload re-posts the stored request unchanged', async () => {
    const stored = record('e-1');
    await seed(stored);
    await restoreUnsentMessages();
    mockedSubmitChat.mockResolvedValueOnce({ event_id: 'e-1' });

    expect(await retryUnsentMessage('e-1')).toBe('sent');

    expect(mockedSubmitChat.mock.calls[0][0]).toEqual(stored.body);
    expect(await storedIds()).toEqual([]);
  });

  it('puts the card where the message was sent, among the thread\'s exchanges', async () => {
    engineRow(LIVE, 1, 'before', '2026-10-03T08:00:00.000Z');
    engineRow(LIVE, 2, 'after', '2026-10-03T08:00:02.000Z');
    await seed(record('e-1', { sentAt: '2026-10-03T08:00:01.000Z' }));

    await restoreUnsentMessages();

    expect(exchangesOf(LIVE).map(exchangeUserMessage)).toEqual(['before', 'message e-1', 'after']);
  });

  it('settles a message the engine already has, with no card', async () => {
    threadMap.value = new Map([['t-first', thread('t-first')]]);
    patchComposeSelection('t-first', { model: 'claude-sonnet-5' });
    engineRow('t-first', 1, 'landed', '2026-10-03T08:00:01.000Z', 'e-1');
    await seed(record('e-1', { threadId: 't-first', settlement: { kind: 'first-send', mode: 'lucidos' } }));

    const outcome = await restoreUnsentMessages();

    expect(outcome.delivered).toBe(1);
    expect(exchangesOf('t-first').filter((ex) => exchangeError(ex))).toEqual([]);
    expect(getComposeSelectionOverride('t-first').model).toBeUndefined();
    expect(await storedIds()).toEqual([]);
  });

  it('settles a restored card once the engine\'s row for it arrives', async () => {
    await seed(record('e-1'));
    await restoreUnsentMessages();

    engineRow(LIVE, 9, 'message e-1', '2026-10-03T08:00:01.000Z', 'e-1');
    settleDeliveredUnsentMessage('e-1');

    expect(exchangesOf(LIVE).filter((ex) => exchangeError(ex))).toEqual([]);
    expect(await storedIds()).toEqual([]);
  });

  it('a second pass brings nothing back twice', async () => {
    await seed(record('e-1'));
    await restoreUnsentMessages();
    await restoreUnsentMessages();
    expect(exchangesOf(LIVE)).toHaveLength(1);
  });
});

describe('a raw new send restored after a reload', () => {
  const rawNew = (eventId: string) => record(eventId, {
    threadId: eventId,
    body: { message: 'a brand-new thread', mode: 'human', event_id: eventId, thread_id: eventId, new_thread: true },
    settlement: { kind: 'raw-new' },
  });

  it('redraws the thread the engine never made, and Retry still creates it', async () => {
    await seed(rawNew('e-new'));
    await restoreUnsentMessages();

    expect(threadMap.value.get('e-new')!.meta.title).toBe('a brand-new thread');
    expect(unsentMessages.value.has('e-new')).toBe(true);
    mockedSubmitChat.mockResolvedValueOnce({ event_id: 'e-new' });
    await retryUnsentMessage('e-new');
    expect(mockedSubmitChat.mock.calls[0][0].new_thread).toBe(true);
  });

  it('counts as delivered when its thread exists, since only it could have made it', async () => {
    vi.mocked(ensureThreadByIdInMap).mockResolvedValue(true);
    await seed(rawNew('e-new'));
    const outcome = await restoreUnsentMessages();
    expect(outcome.delivered).toBe(1);
    expect(unsentMessages.value.size).toBe(0);
    expect(await storedIds()).toEqual([]);
  });

  it('a refused Retry starts a fresh draft with the text', async () => {
    await seed(rawNew('e-new'));
    await restoreUnsentMessages();
    mockedSubmitChat.mockRejectedValueOnce(new ApiError(400, 'bad request'));

    await retryUnsentMessage('e-new');

    expect(getDraft(focusedThreadId.value).text).toBe('a brand-new thread');
  });
});

describe('a record whose thread is gone or could not be checked', () => {
  it('moves the text into a fresh draft and says why, when the thread is gone', async () => {
    vi.mocked(ensureThreadByIdInMap).mockResolvedValue(false);
    await seed(record('e-1', { threadId: 't-gone' }));

    const outcome = await restoreUnsentMessages();

    expect(outcome.movedToDraft).toBe(1);
    expect(getDraft(focusedThreadId.value).text).toBe('message e-1');
    expect(warnings()).toEqual(['An unsent message from before the reload is back in a new draft: its thread no longer exists.']);
    expect(await storedIds()).toEqual([]);
  });

  it('treats a discarded thread as gone', async () => {
    threadMap.value = new Map([['t-discarded', thread('t-discarded', 'discarded')]]);
    await seed(record('e-1', { threadId: 't-discarded' }));
    expect((await restoreUnsentMessages()).movedToDraft).toBe(1);
  });

  it('keeps the record when the engine cannot be asked, and tries again on reconnect', async () => {
    vi.mocked(ensureThreadByIdInMap).mockRejectedValueOnce(new TypeError('Load failed'));
    await seed(record('e-1', { threadId: 't-elsewhere' }));
    threadListFetched.value = true;
    const stop = installUnsentMessageRestore();
    await vi.waitFor(() => expect(warnings()).toHaveLength(1));
    expect(await storedIds()).toEqual(['e-1']);
    expect(warnings()[0]).toMatch(/could not be brought back yet \(Load failed\)/);

    threadMap.value = new Map([...threadMap.value, ['t-elsewhere', thread('t-elsewhere')]]);
    connectionStatus.value = 'disconnected';
    connectionStatus.value = 'connected';
    await vi.waitFor(() => expect(unsentMessages.value.has('e-1')).toBe(true));
    stop();
  });

  it('leaves another open tab\'s records alone', async () => {
    Object.defineProperty(globalThis.navigator, 'locks', {
      configurable: true,
      value: { query: async () => ({ held: [{ name: PAGE_OWNER_LOCK_PREFIX + 'other-tab' }] }), request: async () => {} },
    });
    // Their thread would read as gone here, so a wrong adopt would move it.
    await seed(record('theirs', { ownerId: 'other-tab', threadId: 't-gone' }), record('orphan'));
    await restoreUnsentMessages();
    expect(await storedIds()).toEqual(['orphan', 'theirs']);
    expect([...unsentMessages.value.keys()]).toEqual(['orphan']);
  });
});

describe('a first send restored on a thread the engine still has composing', () => {
  const firstSend = (engineDraftAtSend?: { text: string; imageHashes: string[] }) => record('e-1', {
    threadId: 't-first',
    body: { message: 'the whole message', mode: 'human', event_id: 'e-1', thread_id: 't-first' },
    settlement: { kind: 'first-send', mode: 'lucidos', engineDraftAtSend },
  });

  function engineHasDraft(text: string): void {
    threadMap.value = new Map([['t-first', thread('t-first', 'composing')]]);
    setDraft('t-first', { text, image_hashes: [], mode: 'lucidos' });
    noteServerDraft('t-first', text, []);
  }

  it('shows the message as sent and clears the draft it was written from', async () => {
    engineHasDraft('the whole mess');
    await seed(firstSend({ text: 'the whole mess', imageHashes: [] }));

    await restoreUnsentMessages();

    expect(threadMap.value.get('t-first')!.meta.state).toBe('active');
    expect(getDraft('t-first').text).toBe('');
    expect(exchangeUserMessage(exchangesOf('t-first')[0])).toBe('the whole message');
  });

  it('keeps a draft typed after the send', async () => {
    engineHasDraft('typed after the send');
    await seed(firstSend({ text: 'the whole mess', imageHashes: [] }));

    await restoreUnsentMessages();

    expect(getDraft('t-first').text).toBe('typed after the send');
  });

  it('a refused Retry rolls the draft back with its picks', async () => {
    engineHasDraft('the whole mess');
    patchComposeSelection('t-first', { model: 'claude-sonnet-5' });
    await seed(firstSend({ text: 'the whole mess', imageHashes: [] }));
    await restoreUnsentMessages();
    mockedSubmitChat.mockRejectedValueOnce(new ApiError(409, 'locked'));

    await retryUnsentMessage('e-1');

    expect(threadMap.value.get('t-first')!.meta.state).toBe('composing');
    expect(getDraft('t-first').text).toBe('the whole message');
    expect(getComposeSelectionOverride('t-first').model).toBe('claude-sonnet-5');
  });
});

describe('a follow-up restored after a reload', () => {
  it('a refused Retry takes the text into the draft, after any typing', async () => {
    setDraft(LIVE, { text: 'typed since', image_hashes: [], mode: null });
    await seed(record('e-1'));
    await restoreUnsentMessages();
    mockedSubmitChat.mockRejectedValueOnce(new ApiError(409, 'locked'));

    await retryUnsentMessage('e-1');

    expect(getDraft(LIVE).text).toBe('typed since\n\nmessage e-1');
  });
});

/** A thread-list read: the engine's row for `threadId` in `state`, holding
 *  `composeText` as its draft. */
function summary(threadId: string, state: 'composing' | 'active', composeText: string): ThreadSummary {
  return {
    thread_id: threadId,
    title: '',
    channel: 'chat',
    initiator: 'user',
    created_at: '2026-10-03T07:59:00.000Z',
    last_activity: '2026-10-03T07:59:00.000Z',
    message_count: 0,
    section: 'inbox',
    active_children_count: 0,
    total_children_count: 0,
    blocking_descendant_count: 0,
    attention_descendant_count: 0,
    live_event_wait_count: 0,
    status: 'idle',
    summary_version: 0,
    coding_agent_change_state: { kind: 'none' },
    coding_agent_is_external_repo: false,
    last_revived_at: null,
    state,
    compose_text: composeText,
    compose_images: [],
  };
}

describe('a thread-list read after the restore', () => {
  function read(info: ThreadSummary): void {
    const map = new Map(threadMap.value);
    upsertThread(map, info, false, Date.now() + 1000);
    threadMap.value = map;
  }

  it('leaves a restored first send shown as sent, with no pre-send draft', async () => {
    threadMap.value = new Map([['t-first', thread('t-first', 'composing')]]);
    await seed(record('e-1', { threadId: 't-first', settlement: { kind: 'first-send', mode: 'lucidos' } }));
    await restoreUnsentMessages();

    read(summary('t-first', 'composing', 'the pre-send draft'));

    expect(threadMap.value.get('t-first')!.meta.state).toBe('active');
    expect(getDraft('t-first').text).toBe('');
  });

  it('still takes a peer\'s draft on a thread whose follow-up is unsent', async () => {
    await seed(record('e-1'));
    await restoreUnsentMessages();

    read(summary(LIVE, 'active', 'typed on another device'));

    expect(getDraft(LIVE).text).toBe('typed on another device');
  });
});
