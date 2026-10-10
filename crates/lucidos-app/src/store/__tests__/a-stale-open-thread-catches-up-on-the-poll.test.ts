/**
 * The open thread's transcript catches up on the next health poll when the
 * wake's own catch-up did not land.
 *
 * The reported shape, from an iOS PWA: the header read "Waiting for your
 * answer" over a turn still reading "Working". The steps and the question card
 * that followed were missing. A wake runs two reads side by side. The thread
 * list landed and moved `meta.status`. The open thread's catch-up died in
 * transport on a stale WebKit connection, and gave up silently. Nothing retried
 * it until the reader left the thread and came back.
 *
 * Real `connection` and `thread-loading`, so the stale mark, the coalesce rule
 * and the poll are the code that ships. Only the network is mocked.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { makeThreadState } from '../actions/threads-test-helpers';
import { connectionStatus, focusedThreadId, threadMap, threadsLoaded } from '../store';
import {
  _resetStaleThreadEventsForTesting,
  _resetThreadEventsFailuresForTesting,
  clearThreadFetchGuards,
  markLoadedThreadsStale,
  refreshThreadEvents,
} from '../actions/thread-loading';
import { checkConnection } from '../actions/connection';
import { fetchThreadEvents } from '../../api/threads';
import { ApiError } from '../../api/client';
import type { StoredEvent } from '../thread-events';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadById: vi.fn(),
  fetchThreadEvents: vi.fn(),
  fetchThreadMessages: vi.fn(),
  saveThread: vi.fn().mockResolvedValue(undefined),
  archiveThread: vi.fn().mockResolvedValue({ archived: [] }),
}));

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
  checkHealth: vi.fn().mockResolvedValue({
    status: 'loaded',
    data: { workspace: 'test', workspace_path: '/tmp/test' },
  }),
}));

// The poll opens the event stream when the engine answers, and a test has none.
vi.mock('../actions/thread-sync', () => ({
  connectThreadEvents: vi.fn(),
  disconnectThreadEvents: vi.fn(),
}));

const fetchEvents = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;

const ID = 'open-thread';
const HELD_SEQ = 7;
const QUESTION_SEQ = 9;

async function settle(): Promise<void> {
  for (let i = 0; i < 20; i++) await Promise.resolve();
}

function heldEvents(): Map<number, StoredEvent> {
  return new Map([[HELD_SEQ, { type: 'TriggerStarted', created: '2026-01-01T00:00:00Z' } as StoredEvent]]);
}

beforeEach(() => {
  _resetStaleThreadEventsForTesting();
  _resetThreadEventsFailuresForTesting();
  clearThreadFetchGuards();
  fetchEvents.mockReset();
  connectionStatus.value = 'connected';
  threadsLoaded.value = true;
  threadMap.value = new Map([[ID, makeThreadState(ID, {
    eventsLoaded: true,
    lastDbSeq: HELD_SEQ,
    events: heldEvents(),
  })]]);
  focusedThreadId.value = ID;
});

/** A wake marks the open thread behind, and its catch-up fails twice in
 *  transport: `refreshThreadEvents` retries a transient rejection once. */
async function wakeWhoseCatchUpDies(): Promise<void> {
  markLoadedThreadsStale();
  fetchEvents.mockRejectedValue(new TypeError('Load failed'));
  await refreshThreadEvents(ID, { coalesce: true });
  expect(fetchEvents).toHaveBeenCalledTimes(2);
  // The list read beside it landed, so the header already says the thread waits.
  const thread = threadMap.value.get(ID)!;
  threadMap.value = new Map([[ID, { ...thread, meta: { ...thread.meta, status: 'waiting_for_user_answer' } }]]);
}

describe('an open thread whose wake catch-up did not land', () => {
  it('fetches the missed events on the next health poll', async () => {
    await wakeWhoseCatchUpDies();
    fetchEvents.mockResolvedValue({
      events: [{
        sequence: QUESTION_SEQ,
        event_type: 'UserQuestionAsked',
        payload: { tool_use_id: 'tu-1', question: 'Ship it?', options: [] },
        created: '2026-01-01T00:00:05Z',
        event_id: 'e-9',
      }],
      currentAggregate: null,
    });

    await checkConnection();
    await settle();

    expect(fetchEvents).toHaveBeenLastCalledWith(ID, HELD_SEQ);
    expect(threadMap.value.get(ID)!.events.get(QUESTION_SEQ)?.type).toBe('UserQuestionAsked');
  });

  it('stops asking once a catch-up has landed', async () => {
    await wakeWhoseCatchUpDies();
    fetchEvents.mockResolvedValue({ events: [], currentAggregate: null });

    await checkConnection();
    await settle();
    await checkConnection();
    await settle();

    expect(fetchEvents).toHaveBeenCalledTimes(3);
  });

  it('leaves a refused catch-up to its card rather than re-raising it each tick', async () => {
    markLoadedThreadsStale();
    fetchEvents.mockRejectedValue(new ApiError(500, 'Failed to get thread events'));
    await refreshThreadEvents(ID, { coalesce: true });
    expect(fetchEvents).toHaveBeenCalledTimes(1);

    await checkConnection();
    await settle();

    expect(fetchEvents).toHaveBeenCalledTimes(1);
  });

  it('sends one request per tick for an open thread that is both behind and empty', async () => {
    threadMap.value = new Map([[ID, makeThreadState(ID, { eventsLoaded: true, lastDbSeq: HELD_SEQ })]]);
    markLoadedThreadsStale();
    fetchEvents.mockReturnValue(new Promise(() => {}));

    await checkConnection();
    await settle();

    expect(fetchEvents).toHaveBeenCalledTimes(1);
  });

  it('leaves the empty-thread recovery its own request on each budgeted tick', async () => {
    // A slow refresh still in flight must not use up the recovery's three
    // attempts, which exist to ask again after the backend has committed.
    const EMPTY = 'empty-thread';
    threadMap.value = new Map([[EMPTY, makeThreadState(EMPTY, { eventsLoaded: true })]]);
    focusedThreadId.value = EMPTY;
    fetchEvents.mockReturnValue(new Promise(() => {}));

    for (let tick = 0; tick < 3; tick++) {
      await checkConnection();
      await settle();
    }

    expect(fetchEvents).toHaveBeenCalledTimes(3);
  });

  it('asks nothing for an open thread that is already current', async () => {
    fetchEvents.mockResolvedValue({ events: [], currentAggregate: null });

    await checkConnection();
    await settle();

    expect(fetchEvents).not.toHaveBeenCalled();
  });
});
