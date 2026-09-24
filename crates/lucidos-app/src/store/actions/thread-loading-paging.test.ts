import { describe, it, expect, beforeEach, vi } from 'vitest';

// Polyfill localStorage before store.ts is imported at module level. Mirrors
// the scaffolding in thread-loading-stale.test.ts, which drives the same module.
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

import { makeThreadState } from './threads-test-helpers';
import { threadMap, toasts } from '../store';
import {
  clearThreadFetchGuards,
  ensureWholeThreadLoaded,
  loadOlderThreadEvents,
  _resetHistoryReadsForTesting,
  loadThreadEvents,
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
  composeHandlers: vi.fn(),
}));

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
}));

const fetchEvents = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;

const THREAD = 'thread-paging';

/** A persisted row as the snapshot endpoint serves one. */
function row(sequence: number, created: string) {
  return {
    sequence,
    event_type: 'TextStreamed',
    payload: { text: `row-${sequence}` },
    created,
    event_id: `evt-${sequence}`,
  };
}

/** N rows, ascending, one second apart, as a page arrives. */
function page(from: number, count: number) {
  return Array.from({ length: count }, (_, i) =>
    row(from + i, new Date(Date.UTC(2026, 0, 1, 0, 0, from + i)).toISOString()));
}

/** Both fire-and-forget, so let the promise chains settle before asserting. */
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
    // The OLDEST row of the page is the floor: it is what a backfill pages back from.
    expect(thread.historyFloor).toEqual({ created: rows[0].created, sequence: 100 });
    expect(thread.hasOlderEvents).toBe(true);
  });

  it('takes the FORWARD watermark from the server, not from the page', async () => {
    // A sequence is allocated globally and can run against the clock. An
    // unseen older row may hold a higher one than anything in the page, and
    // half the threads in the reporting workspace are like that. Deriving the
    // watermark from the page would make the next delta refetch history and
    // replay it through the live path.
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
    // The median thread is 198 events. It fits in a page, and must behave as it
    // always did: one request, and no backfill machinery ever engaged.
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
    // `groupIntoExchangesCached` memoises on the Map object and finds new work
    // by insertion-order suffix. Rows belonging at the START must arrive as a
    // new object, or the fold serves stale exchanges with no failure signal.
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
    // `lastDbSeq` is the high-water mark of the NEWEST event seen. This page is
    // older than everything held, so moving it would claim a gap that is not there.
    expect(thread.lastDbSeq).toBe(seqBefore);
    expect(thread.hasOlderEvents).toBe(false);
  });

  it('holds a fetched page until its landing gate opens, and stays in flight meanwhile', async () => {
    // The transcript's gate while the reader holds its scrollbar: a held drag
    // undoes the anchor write that would keep them still (ADR 0258).
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
    // The read starts on a microtask, so let it reach the fetch before counting.
    await Promise.resolve();
    expect(fetchEvents).toHaveBeenCalledTimes(1);

    release({ events: page(50, 1), currentAggregate: null, hasMore: false });
    await first;
    await settle();
  });

  it('toasts a failure and stays willing to retry', async () => {
    // Scrolling up IS user intent, so a silent failure would leave the
    // transcript simply stopping. The flag survives, so the next scroll asks again.
    fetchEvents.mockRejectedValue(new Error('offline'));

    expect(await loadOlderThreadEvents(THREAD)).toBe(false);
    await settle();

    const thread = threadMap.value.get(THREAD)!;
    expect(thread.hasOlderEvents).toBe(true);
    expect(toasts.value.some(t => t.type === 'error')).toBe(true);
    // And the next scroll really does try again, rather than finding a flag
    // some failed attempt left set.
    fetchEvents.mockResolvedValue({ events: page(50, 1), currentAggregate: null, hasMore: false });
    expect(await loadOlderThreadEvents(THREAD)).toBe(true);
  });

  it('retries a transport failure once before telling the reader', async () => {
    // The first request after iOS resumes the PWA often dies on a stale
    // connection and never reaches the engine. A second attempt succeeds.
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

    // An event that arrived over SSE after the first page, so the unpaged
    // response below cannot know about it.
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
    // A deep link arriving mid-scroll used to find the busy flag set and give
    // up. Nobody then fetched the history it needed, and the link landed on a
    // transcript without its target.
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);
    fetchEvents.mockResolvedValue({ events: page(100, 2), currentAggregate: null, hasMore: true });
    await loadThreadEvents(THREAD);
    await settle();
    fetchEvents.mockReset();

    let releaseBackfill: (v: unknown) => void = () => {};
    fetchEvents.mockReturnValueOnce(new Promise((r) => { releaseBackfill = r; }));
    const backfill = loadOlderThreadEvents(THREAD);

    // The whole-thread load starts while the backfill is still in flight.
    fetchEvents.mockResolvedValue({ events: page(1, 6), currentAggregate: null });
    const whole = ensureWholeThreadLoaded(THREAD);

    releaseBackfill({ events: page(50, 2), currentAggregate: null, hasMore: true });
    await backfill;
    await whole;
    await settle();

    // It ran after the backfill, rather than returning without fetching.
    expect(fetchEvents).toHaveBeenCalledWith(THREAD);
    expect(threadMap.value.get(THREAD)!.hasOlderEvents).toBe(false);
  });

  it('is safe to ask twice, which is what lets a caller retry on a cold open', async () => {
    // A deep link opens a cold thread and asks before the first page has
    // landed, when the thread still reports nothing older. The second ask,
    // once the load settles the answer, is the one that fetches.
    threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { eventsLoaded: false })]]);

    // Too early: nothing is loaded, so nothing is known to be missing.
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
    // Two callers queued behind the same in-flight read used to resume
    // together and each start their own. Two unbounded fetches for a long
    // thread is what the serialiser exists to prevent.
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

    // The backfill, then ONE whole-history read. The second caller found the
    // thread already whole and asked for nothing.
    const unpaged = fetchEvents.mock.calls.filter(c => c[1] === undefined);
    expect(unpaged).toHaveLength(1);
  });

  it('retries a transport failure once before telling the reader', async () => {
    // The reported case: the up chevron's read died on a stale connection
    // right after a resume, and toasted with nothing lost but one request.
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
