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
});

import { makeThreadAggregate, makeThreadState } from './threads-test-helpers';
import { connectionStatus, engineRestarting, threadMap, toasts } from '../store';
import {
  clearThreadFetchGuards,
  ensureWholeThreadLoaded,
  loadOlderThreadEvents,
  _resetHistoryReadsForTesting,
  loadThreadEvents,
  refreshThreadEvents,
  THREAD_EVENTS_PAGE_SIZE,
  _resetThreadEventsFailuresForTesting,
} from './thread-loading';
import { fetchThreadEvents } from '../../api/threads';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadById: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
  fetchThreadMessages: vi.fn(),
  saveThread: vi.fn().mockResolvedValue(undefined),
  archiveThread: vi.fn().mockResolvedValue({ archived: [] }),
}));

vi.mock('../../components/chat/promptFocus', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../components/chat/promptFocus')>()),
  focusPromptNow: vi.fn(),
}));

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
}));

const fetchEvents = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;

const THREAD = 'thread-paging';

function row(sequence: number, created: string) {
  return {
    sequence,
    event_type: 'TextStreamed',
    payload: { text: `row-${sequence}` },
    created,
    event_id: `evt-${sequence}`,
  };
}

function page(from: number, count: number) {
  return Array.from({ length: count }, (_, i) =>
    row(from + i, new Date(Date.UTC(2026, 0, 1, 0, 0, from + i)).toISOString()));
}

async function settle(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}

beforeEach(() => {
  threadMap.value = new Map();
  toasts.value = [];
  clearThreadFetchGuards();
  _resetThreadEventsFailuresForTesting();
  _resetHistoryReadsForTesting();
  fetchEvents.mockReset();
});

describe('a cold open takes a page', () => {
  it('asks for the newest page rather than the whole history', async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    fetchEvents.mockResolvedValue({ events: page(100, 3), currentAggregate: null, hasMore: true });

    await loadThreadEvents(THREAD);
    await settle();

    expect(fetchEvents).toHaveBeenCalledWith(THREAD, { limit: THREAD_EVENTS_PAGE_SIZE });
  });

  it('records the floor and that older events remain', async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    const rows = page(100, 3);
    fetchEvents.mockResolvedValue({ events: rows, currentAggregate: null, hasMore: true });

    await loadThreadEvents(THREAD);
    await settle();

    const thread = threadMap.value.get(THREAD)!;
    expect(thread.historyFloor).toEqual({ created: rows[0].created, sequence: 100 });
    expect(thread.hasOlderEvents).toBe(true);
  });

  it('takes the FORWARD watermark from the server, not from the page', async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    fetchEvents.mockResolvedValue({
      events: page(100, 3),
      currentAggregate: null,
      hasMore: true,
      maxSequence: 9001,
    });

    await loadThreadEvents(THREAD);
    await settle();

    expect(threadMap.value.get(THREAD)!.lastDbSeq).toBe(9001);
  });

  it('settles a SHORT thread on nothing older, so it never asks again', async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    fetchEvents.mockResolvedValue({ events: page(1, 3), currentAggregate: null });

    await loadThreadEvents(THREAD);
    await settle();

    const thread = threadMap.value.get(THREAD)!;
    expect(thread.hasOlderEvents).toBe(false);

    fetchEvents.mockClear();
    expect(await loadOlderThreadEvents(THREAD)).toBe(false);
    expect(fetchEvents).not.toHaveBeenCalled();
  });
});

describe('a backfill takes the page behind the floor', () => {
  beforeEach(async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    fetchEvents.mockResolvedValue({ events: page(100, 3), currentAggregate: null, hasMore: true });
    await loadThreadEvents(THREAD);
    await settle();
    fetchEvents.mockReset();
  });

  it('pages back from the floor it recorded', async () => {
    fetchEvents.mockResolvedValue({ events: page(50, 3), currentAggregate: null, hasMore: true });

    await loadOlderThreadEvents(THREAD);
    await settle();

    expect(fetchEvents).toHaveBeenCalledWith(THREAD, {
      limit: THREAD_EVENTS_PAGE_SIZE,
      before: { created: page(100, 1)[0].created, sequence: 100 },
    });
  });

  it('puts the older events IN FRONT, and keeps the ones already held', async () => {
    fetchEvents.mockResolvedValue({ events: page(50, 3), currentAggregate: null, hasMore: false });

    await loadOlderThreadEvents(THREAD);
    await settle();

    const thread = threadMap.value.get(THREAD)!;
    expect([...thread.events.keys()]).toEqual([50, 51, 52, 100, 101, 102]);
  });

  it('REPLACES the events Map, which the incremental fold contract requires', async () => {
    const before = threadMap.value.get(THREAD)!.events;
    fetchEvents.mockResolvedValue({ events: page(50, 2), currentAggregate: null, hasMore: false });

    await loadOlderThreadEvents(THREAD);
    await settle();

    expect(threadMap.value.get(THREAD)!.events).not.toBe(before);
  });

  it('moves the floor back and leaves lastDbSeq alone', async () => {
    const seqBefore = threadMap.value.get(THREAD)!.lastDbSeq;
    const older = page(50, 3);
    fetchEvents.mockResolvedValue({ events: older, currentAggregate: null, hasMore: false });

    await loadOlderThreadEvents(THREAD);
    await settle();

    const thread = threadMap.value.get(THREAD)!;
    expect(thread.historyFloor).toEqual({ created: older[0].created, sequence: 50 });
    expect(thread.lastDbSeq).toBe(seqBefore);
    expect(thread.hasOlderEvents).toBe(false);
  });

  it('holds a fetched page until its landing gate opens, and stays in flight meanwhile', async () => {
    let open: () => void = () => {};
    const gate = new Promise<void>((r) => { open = r; });
    fetchEvents.mockResolvedValue({ events: page(50, 3), currentAggregate: null, hasMore: false });

    const read = loadOlderThreadEvents(THREAD, THREAD_EVENTS_PAGE_SIZE, () => gate);
    await settle();
    expect([...threadMap.value.get(THREAD)!.events.keys()]).toEqual([100, 101, 102]);
    expect(await loadOlderThreadEvents(THREAD)).toBe(false);

    open();
    expect(await read).toBe(true);
    expect([...threadMap.value.get(THREAD)!.events.keys()]).toEqual([50, 51, 52, 100, 101, 102]);
  });

  it('runs one at a time, so a fast scroll cannot double-fetch', async () => {
    let release: (v: unknown) => void = () => {};
    fetchEvents.mockReturnValue(new Promise((r) => { release = r; }));

    const first = loadOlderThreadEvents(THREAD);
    const second = await loadOlderThreadEvents(THREAD);
    expect(second).toBe(false);
    await Promise.resolve();
    expect(fetchEvents).toHaveBeenCalledTimes(1);

    release({ events: page(50, 1), currentAggregate: null, hasMore: false });
    await first;
    await settle();
  });

  it('toasts a failure and stays willing to retry', async () => {
    fetchEvents.mockRejectedValue(new Error('offline'));

    expect(await loadOlderThreadEvents(THREAD)).toBe(false);
    await settle();

    const thread = threadMap.value.get(THREAD)!;
    expect(thread.hasOlderEvents).toBe(true);
    expect(toasts.value.some(t => t.type === 'error')).toBe(true);
    fetchEvents.mockResolvedValue({ events: page(50, 1), currentAggregate: null, hasMore: false });
    expect(await loadOlderThreadEvents(THREAD)).toBe(true);
  });

  it('retries a transport failure once before telling the reader', async () => {
    fetchEvents
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValue({ events: page(50, 1), currentAggregate: null, hasMore: false });

    expect(await loadOlderThreadEvents(THREAD)).toBe(true);
    expect(fetchEvents).toHaveBeenCalledTimes(2);
    expect(toasts.value).toEqual([]);
  });
});

describe('a whole-thread surface loads the whole thread', () => {
  it('fetches unpaged, and merges rather than dropping a live arrival', async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    fetchEvents.mockResolvedValue({ events: page(100, 2), currentAggregate: null, hasMore: true });
    await loadThreadEvents(THREAD);
    await settle();

    const thread = threadMap.value.get(THREAD)!;
    thread.events.set(200, { type: 'TextStreamed', created: '2026-01-02T00:00:00Z' } as never);

    fetchEvents.mockReset();
    fetchEvents.mockResolvedValue({ events: page(1, 4), currentAggregate: null });
    await ensureWholeThreadLoaded(THREAD);
    await settle();

    expect(fetchEvents).toHaveBeenCalledWith(THREAD);
    const after = threadMap.value.get(THREAD)!;
    expect(after.events.has(200)).toBe(true);
    expect(after.hasOlderEvents).toBe(false);
  });

  it('holds the whole history until its landing gate opens', async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    fetchEvents.mockResolvedValue({ events: page(100, 2), currentAggregate: null, hasMore: true });
    await loadThreadEvents(THREAD);
    await settle();

    let open: () => void = () => {};
    const gate = new Promise<void>((r) => { open = r; });
    fetchEvents.mockReset();
    fetchEvents.mockResolvedValue({ events: page(1, 4), currentAggregate: null });
    const read = ensureWholeThreadLoaded(THREAD, () => gate);
    await settle();
    expect(threadMap.value.get(THREAD)!.hasOlderEvents).toBe(true);

    open();
    expect(await read).toBe(true);
    expect(threadMap.value.get(THREAD)!.hasOlderEvents).toBe(false);
  });

  it('WAITS for a backfill rather than skipping past one', async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    fetchEvents.mockResolvedValue({ events: page(100, 2), currentAggregate: null, hasMore: true });
    await loadThreadEvents(THREAD);
    await settle();
    fetchEvents.mockReset();

    let releaseBackfill: (v: unknown) => void = () => {};
    fetchEvents.mockReturnValueOnce(new Promise((r) => { releaseBackfill = r; }));
    const backfill = loadOlderThreadEvents(THREAD);

    fetchEvents.mockResolvedValue({ events: page(1, 6), currentAggregate: null });
    const whole = ensureWholeThreadLoaded(THREAD);

    releaseBackfill({ events: page(50, 2), currentAggregate: null, hasMore: true });
    await backfill;
    await whole;
    await settle();

    expect(fetchEvents).toHaveBeenCalledWith(THREAD);
    expect(threadMap.value.get(THREAD)!.hasOlderEvents).toBe(false);
  });

  it('is safe to ask twice, which is what lets a caller retry on a cold open', async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);

    await ensureWholeThreadLoaded(THREAD);
    expect(fetchEvents).not.toHaveBeenCalled();

    fetchEvents.mockResolvedValue({ events: page(100, 2), currentAggregate: null, hasMore: true });
    await loadThreadEvents(THREAD);
    await settle();
    fetchEvents.mockReset();

    fetchEvents.mockResolvedValue({ events: page(1, 8), currentAggregate: null });
    await ensureWholeThreadLoaded(THREAD);
    await settle();
    expect(fetchEvents).toHaveBeenCalledWith(THREAD);
    expect(threadMap.value.get(THREAD)!.hasOlderEvents).toBe(false);
  });

  it('fetches the whole history ONCE when two callers queue behind one read', async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    fetchEvents.mockResolvedValue({ events: page(100, 2), currentAggregate: null, hasMore: true });
    await loadThreadEvents(THREAD);
    await settle();
    fetchEvents.mockReset();

    let release: (v: unknown) => void = () => {};
    fetchEvents.mockReturnValueOnce(new Promise((r) => { release = r; }));
    const backfill = loadOlderThreadEvents(THREAD);

    fetchEvents.mockResolvedValue({ events: page(1, 8), currentAggregate: null });
    const first = ensureWholeThreadLoaded(THREAD);
    const second = ensureWholeThreadLoaded(THREAD);

    release({ events: page(50, 2), currentAggregate: null, hasMore: true });
    await Promise.all([backfill, first, second]);
    await settle();

    const unpaged = fetchEvents.mock.calls.filter(c => c[1] === undefined);
    expect(unpaged).toHaveLength(1);
  });

  it('retries a transport failure once before telling the reader', async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    fetchEvents.mockResolvedValue({ events: page(100, 2), currentAggregate: null, hasMore: true });
    await loadThreadEvents(THREAD);
    await settle();
    fetchEvents.mockReset();

    fetchEvents
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValue({ events: page(1, 4), currentAggregate: null });

    expect(await ensureWholeThreadLoaded(THREAD)).toBe(true);
    expect(fetchEvents).toHaveBeenCalledTimes(2);
    expect(toasts.value).toEqual([]);
  });

  it('toasts a failure the retry does not cure, and stays willing to retry', async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    fetchEvents.mockResolvedValue({ events: page(100, 2), currentAggregate: null, hasMore: true });
    await loadThreadEvents(THREAD);
    await settle();
    fetchEvents.mockReset();

    fetchEvents.mockRejectedValue(new TypeError('Load failed'));

    expect(await ensureWholeThreadLoaded(THREAD)).toBe(false);
    expect(fetchEvents).toHaveBeenCalledTimes(2);
    expect(toasts.value.some(t => t.type === 'error')).toBe(true);
    expect(threadMap.value.get(THREAD)!.hasOlderEvents).toBe(true);
  });

  it('is a no-op on a thread already loaded to its start', async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    fetchEvents.mockResolvedValue({ events: page(1, 2), currentAggregate: null });
    await loadThreadEvents(THREAD);
    await settle();

    fetchEvents.mockClear();
    await ensureWholeThreadLoaded(THREAD);
    expect(fetchEvents).not.toHaveBeenCalled();
  });
});

describe('a cold load lands on the thread the map holds, under the rules a restart and a retry set', () => {
  it('applies a landed page to the state the map holds after the fetch, not the one it started with', async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    const replacement = makeThreadState(THREAD, { eventsLoaded: false });
    fetchEvents.mockImplementationOnce(async () => {
      threadMap.value = new Map([[THREAD, replacement]]);
      return { events: page(100, 2), currentAggregate: null, hasMore: false };
    });

    await loadThreadEvents(THREAD);
    await settle();

    expect(replacement.eventsLoaded).toBe(true);
    expect(threadMap.value.get(THREAD)).toBe(replacement);
  });

  it('a load that lands clears a failure another attempt set while it was in flight', async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    fetchEvents.mockImplementationOnce(async () => {
      threadMap.value.get(THREAD)!.eventsLoadFailed = true;
      return { events: page(100, 2), currentAggregate: null, hasMore: false };
    });

    await loadThreadEvents(THREAD);
    await settle();

    const thread = threadMap.value.get(THREAD)!;
    expect(thread.eventsLoaded).toBe(true);
    expect(thread.eventsLoadFailed).toBe(false);
  });

  it('an engine restart mid-load leaves the thread flagged failed, so the resume sync still finds it', async () => {
    vi.useFakeTimers();
    try {
      threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
      fetchEvents.mockImplementation(async () => {
        engineRestarting.value = true;
        throw new Error('engine down');
      });

      const load = loadThreadEvents(THREAD);
      await vi.runAllTimersAsync();
      await load;

      expect(threadMap.value.get(THREAD)!.eventsLoadFailed).toBe(true);
      expect(toasts.value).toEqual([]);
    } finally {
      engineRestarting.value = false;
      vi.useRealTimers();
    }
  });

  it('a transient abort during a load is retried, and the reader is not told', async () => {
    vi.useFakeTimers();
    try {
      threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
      const aborted = Object.assign(new Error('aborted'), { name: 'AbortError' });
      fetchEvents
        .mockRejectedValueOnce(aborted)
        .mockResolvedValue({ events: page(100, 2), currentAggregate: null, hasMore: false });

      const load = loadThreadEvents(THREAD);
      await vi.runAllTimersAsync();
      await load;

      expect(threadMap.value.get(THREAD)!.eventsLoaded).toBe(true);
      expect(toasts.value).toEqual([]);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('a refresh failure card names the thread that failed last', () => {
  beforeEach(() => {
    connectionStatus.value = 'connected';
    threadMap.value = new Map([
      ['a', makeThreadState('a', { eventsLoaded: true })],
      ['b', makeThreadState('b', { eventsLoaded: true })],
    ]);
  });

  it('shows the newest reason when one thread fails again after another failed', async () => {
    fetchEvents
      .mockRejectedValueOnce(new Error('reason 1'))
      .mockRejectedValueOnce(new Error('reason 2'))
      .mockRejectedValueOnce(new Error('reason 3'));

    await refreshThreadEvents('a');
    await refreshThreadEvents('b');
    await refreshThreadEvents('a');

    const text = toasts.value.map(t => t.message).join(' | ');
    expect(text).toContain('reason 3');
    expect(text).not.toContain('reason 2');
  });

  it('a recovery does not re-raise a card the reader dismissed', async () => {
    fetchEvents
      .mockRejectedValueOnce(new Error('a broke'))
      .mockRejectedValueOnce(new Error('b broke'));
    await refreshThreadEvents('a');
    await refreshThreadEvents('b');
    expect(toasts.value.length).toBeGreaterThan(0);

    toasts.value = [];
    fetchEvents.mockResolvedValueOnce({ events: [], currentAggregate: null });
    await refreshThreadEvents('a');

    expect(toasts.value).toEqual([]);
  });
});

describe('a cold load folds its snapshot in the order the thread meta needs', () => {
  it('the snapshot aggregate has the last word on the thread meta, over whatever the replayed rows moved', async () => {
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    fetchEvents.mockResolvedValueOnce({
      events: [{ sequence: 1, event_type: 'TextStreamed', payload: { text: 'x' }, created: '2026-06-02T00:00:00Z', event_id: 'evt-1' }],
      currentAggregate: makeThreadAggregate(THREAD, { lastActivity: '2026-06-01T00:00:00Z' }),
      hasMore: false,
    });

    await loadThreadEvents(THREAD);
    await settle();

    expect(threadMap.value.get(THREAD)!.meta.updatedAt).toBe('2026-06-01T00:00:00Z');
  });

  it('replaying a past call leaves the live call rows standing as they were', async () => {
    const live = { eventId: 'live-1', count: 1, created: '2026-06-01T00:00:00Z', text: 'current words' };
    const thread = makeThreadState(THREAD, { eventsLoaded: false });
    thread.liveUtterances = [live];
    threadMap.value = new Map([[THREAD, thread]]);
    fetchEvents.mockResolvedValueOnce({
      events: [{ sequence: 1, event_type: 'VoiceSessionEnded', payload: { voice_session_id: 'old-call' }, created: '2026-05-01T00:00:00Z', event_id: 'evt-old' }],
      currentAggregate: null,
      hasMore: false,
    });

    await loadThreadEvents(THREAD);
    await settle();

    expect(threadMap.value.get(THREAD)!.liveUtterances).toEqual([live]);
  });
});

describe('a cold load that fails after the thread has landed', () => {
  it('raises no card, since the thread already has its history', async () => {
    vi.useFakeTimers();
    try {
      threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
      fetchEvents.mockImplementation(async () => {
        threadMap.value.get(THREAD)!.eventsLoaded = true;
        throw new Error('boom');
      });

      const load = loadThreadEvents(THREAD);
      await vi.runAllTimersAsync();
      await load;

      expect(toasts.value).toEqual([]);
    } finally {
      vi.useRealTimers();
    }
  });
});
