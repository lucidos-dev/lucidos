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

import { makeThreadState } from './threads-test-helpers';
import { connectionStatus, focusedThreadId, threadMap, toasts } from '../store';
import { _resetComposeDraftsForTesting } from '../composeDrafts';
import {
  _resetStaleThreadEventsForTesting,
  _resetThreadEventsFailuresForTesting,
  clearThreadFetchGuards,
  loadOlderThreadEvents,
  loadThreadEvents,
  markLoadedThreadsStale,
  refreshStaleThreadEvents,
  refreshThreadEvents,
  threadEventsStillArriving,
  threadLoadInFlightMs,
} from './thread-loading';
import { focusThread } from './threads';
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

async function settle(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}

function putThreads(states: Array<[string, ReturnType<typeof makeThreadState>]>): void {
  threadMap.value = new Map(states);
}

function loaded(id: string) {
  return makeThreadState(id, { eventsLoaded: true, lastDbSeq: 7 });
}

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
  connectionStatus.value = 'connected';
  toasts.value = [];
  _resetComposeDraftsForTesting();
  _resetStaleThreadEventsForTesting();
  _resetThreadEventsFailuresForTesting();
  clearThreadFetchGuards();
  fetchEvents.mockReset();
  fetchEvents.mockResolvedValue({ events: [], currentAggregate: null });
  localStorage.removeItem('lucidos-focused-thread');
});

describe('markLoadedThreadsStale', () => {
  it('marks only threads whose events are loaded', async () => {
    putThreads([
      ['has-events', loaded('has-events')],
      ['never-loaded', makeThreadState('never-loaded')],
    ]);

    markLoadedThreadsStale();
    refreshStaleThreadEvents('has-events');
    refreshStaleThreadEvents('never-loaded');
    await settle();

    expect(fetchEvents.mock.calls.map(c => c[0])).toEqual(['has-events']);
  });

  it('catches the open thread up when its first load lands after a gap opened', async () => {
    putThreads([['opening', makeThreadState('opening')]]);
    focusedThreadId.value = 'opening';
    let answer!: (snapshot: { events: never[]; currentAggregate: null }) => void;
    fetchEvents.mockImplementationOnce(() => new Promise((resolve) => { answer = resolve; }));
    const load = loadThreadEvents('opening');
    await settle();

    markLoadedThreadsStale();
    answer({ events: [], currentAggregate: null });
    await load;
    await settle();

    expect(fetchEvents.mock.calls.map(c => c[0])).toEqual(['opening', 'opening']);
  });

  it('rebuilds the set, so a departed thread leaves no entry behind', async () => {
    putThreads([['gone', loaded('gone')], ['stays', loaded('stays')]]);
    markLoadedThreadsStale();

    const next = new Map(threadMap.value);
    next.delete('gone');
    threadMap.value = next;
    markLoadedThreadsStale();

    const back = new Map(threadMap.value);
    back.set('gone', loaded('gone'));
    threadMap.value = back;
    refreshStaleThreadEvents('gone');
    await settle();

    expect(fetchEvents).not.toHaveBeenCalled();
  });

  it('survives clearThreadFetchGuards, which runs where the marks are set', async () => {
    putThreads([['t1', loaded('t1')]]);
    markLoadedThreadsStale();

    clearThreadFetchGuards();
    refreshStaleThreadEvents('t1');
    await settle();

    expect(fetchEvents).toHaveBeenCalledTimes(1);
  });
});

describe('refreshStaleThreadEvents', () => {
  it('issues nothing for a thread that was never marked', async () => {
    putThreads([['t1', loaded('t1')]]);

    refreshStaleThreadEvents('t1');
    await settle();

    expect(fetchEvents).not.toHaveBeenCalled();
  });

  it('is consumed by a landed refresh, so a second open issues nothing', async () => {
    putThreads([['t1', loaded('t1')]]);
    markLoadedThreadsStale();

    refreshStaleThreadEvents('t1');
    await settle();
    refreshStaleThreadEvents('t1');
    await settle();

    expect(fetchEvents).toHaveBeenCalledTimes(1);
  });

  it('keeps the mark when the refresh did not land, so re-opening retries', async () => {
    putThreads([['t1', loaded('t1')]]);
    markLoadedThreadsStale();
    fetchEvents.mockRejectedValue(new DOMException('aborted', 'AbortError'));

    refreshStaleThreadEvents('t1');
    await settle();
    expect(fetchEvents).toHaveBeenCalledTimes(2);

    fetchEvents.mockResolvedValue({ events: [], currentAggregate: null });
    refreshStaleThreadEvents('t1');
    await settle();

    expect(fetchEvents).toHaveBeenCalledTimes(3);
  });

  it('keeps the mark set by a NEWER sync point when an older refresh lands', async () => {
    putThreads([['t1', loaded('t1')]]);
    markLoadedThreadsStale();

    let release!: (v: unknown) => void;
    fetchEvents.mockReturnValueOnce(new Promise(r => { release = r; }));
    refreshStaleThreadEvents('t1');
    await settle();

    markLoadedThreadsStale();
    release({ events: [], currentAggregate: null });
    await settle();

    fetchEvents.mockResolvedValue({ events: [], currentAggregate: null });
    refreshStaleThreadEvents('t1');
    await settle();

    expect(fetchEvents).toHaveBeenCalledTimes(2);
  });

  it('does not coalesce into a refresh that started before the mark', async () => {
    putThreads([['t1', loaded('t1')]]);

    let release!: (v: unknown) => void;
    fetchEvents.mockReturnValueOnce(new Promise(r => { release = r; }));
    void refreshThreadEvents('t1');
    markLoadedThreadsStale();
    refreshStaleThreadEvents('t1');
    await settle();

    expect(fetchEvents).toHaveBeenCalledTimes(2);

    release({ events: [], currentAggregate: null });
    await settle();
    refreshStaleThreadEvents('t1');
    await settle();

    expect(fetchEvents).toHaveBeenCalledTimes(2);
  });

  it('still coalesces into a refresh that started after the mark', async () => {
    putThreads([['t1', loaded('t1')]]);
    markLoadedThreadsStale();

    let release!: (v: unknown) => void;
    fetchEvents.mockReturnValueOnce(new Promise(r => { release = r; }));
    refreshStaleThreadEvents('t1');
    refreshStaleThreadEvents('t1');
    await settle();

    expect(fetchEvents).toHaveBeenCalledTimes(1);
    release({ events: [], currentAggregate: null });
    await settle();
  });

  it('is consumed by a landed full LOAD, which subsumes a refresh', async () => {
    putThreads([['t1', loaded('t1')]]);
    markLoadedThreadsStale();
    threadMap.value = new Map([['t1', makeThreadState('t1')]]);

    await loadThreadEvents('t1');
    await settle();
    expect(fetchEvents).toHaveBeenCalledTimes(1);

    refreshStaleThreadEvents('t1');
    await settle();

    expect(fetchEvents).toHaveBeenCalledTimes(1);
  });
});

describe('focusThread', () => {
  it('catches up a thread a sync point marked stale', async () => {
    putThreads([['t1', loaded('t1')], ['t2', loaded('t2')]]);
    markLoadedThreadsStale();

    focusThread('t2');
    await settle();

    expect(fetchEvents.mock.calls).toEqual([['t2', 7]]);
  });

  it('issues no request for a thread that is already current', async () => {
    putThreads([['t1', loaded('t1')]]);

    focusThread('t1');
    await settle();

    expect(fetchEvents).not.toHaveBeenCalled();
  });
});

describe('threadEventsStillArriving', () => {
  it('is true for a thread whose events have never loaded', () => {
    putThreads([['t1', makeThreadState('t1')]]);
    expect(threadEventsStillArriving('t1')).toBe(true);
  });

  it('is true through a load that is still retrying', async () => {
    putThreads([['t1', makeThreadState('t1')]]);
    fetchEvents.mockRejectedValue(new Error('boom'));

    void loadThreadEvents('t1');
    await settle();

    expect(threadEventsStillArriving('t1')).toBe(true);
  });

  it('is true with no claim standing, when the events are simply not loaded', async () => {
    putThreads([['t1', makeThreadState('t1')]]);
    fetchEvents.mockImplementation(() => new Promise(() => {}));
    void loadThreadEvents('t1');
    await settle();

    clearThreadFetchGuards();

    expect(threadEventsStillArriving('t1')).toBe(true);
  });

  it('is true while a REFRESH catches up a thread already loaded', async () => {
    putThreads([['t1', loaded('t1')]]);
    let release: (() => void) | null = null;
    fetchEvents.mockImplementation(() => new Promise((resolve) => {
      release = () => resolve({ events: [], currentAggregate: null });
    }));
    markLoadedThreadsStale();

    void refreshThreadEvents('t1');
    expect(threadEventsStillArriving('t1')).toBe(true);

    release!();
    await settle();
    expect(threadEventsStillArriving('t1')).toBe(false);
  });

  it('is true while a page of older history is on its way', async () => {
    putThreads([['t1', {
      ...loaded('t1'), hasOlderEvents: true, historyFloor: { created: '2026-01-01T00:00:00Z', sequence: 1 },
    }]]);
    let release: (() => void) | null = null;
    fetchEvents.mockImplementation(() => new Promise((resolve) => {
      release = () => resolve({ events: [], currentAggregate: null });
    }));

    const page = loadOlderThreadEvents('t1');
    await settle();
    expect(threadEventsStillArriving('t1')).toBe(true);

    release!();
    await page;
    expect(threadEventsStillArriving('t1')).toBe(false);
  });

  it('is false once the events are loaded and nothing is in flight', () => {
    putThreads([['t1', loaded('t1')]]);
    expect(threadEventsStillArriving('t1')).toBe(false);
  });

  it('is false when the load gave up and said so', () => {
    putThreads([['t1', makeThreadState('t1', { eventsLoadFailed: true })]]);
    expect(threadEventsStillArriving('t1')).toBe(false);
  });

  it('is false for a thread that has left the map', () => {
    expect(threadEventsStillArriving('gone')).toBe(false);
  });
});

describe('threadLoadInFlightMs', () => {
  it('is null when nothing is in flight', () => {
    putThreads([['t1', makeThreadState('t1')]]);
    expect(threadLoadInFlightMs('t1')).toBeNull();
  });

  it('is null for a thread that has left the map', () => {
    expect(threadLoadInFlightMs('gone')).toBeNull();
  });

  it('is an elapsed time while a load is running', async () => {
    putThreads([['t1', makeThreadState('t1')]]);
    fetchEvents.mockImplementation(() => new Promise(() => {}));
    void loadThreadEvents('t1');
    await settle();

    const elapsed = threadLoadInFlightMs('t1');
    expect(elapsed).not.toBeNull();
    expect(elapsed).toBeGreaterThanOrEqual(0);
  });

  it('spans the retry backoff, not just one attempt', async () => {
    putThreads([['t1', makeThreadState('t1')]]);
    fetchEvents.mockRejectedValue(new Error('boom'));
    void loadThreadEvents('t1');
    await settle();

    expect(threadLoadInFlightMs('t1')).not.toBeNull();
  });

  it('goes back to null once the load settles', async () => {
    putThreads([['t1', makeThreadState('t1')]]);
    fetchEvents.mockResolvedValue({ events: [], currentAggregate: null });
    await loadThreadEvents('t1');

    expect(threadLoadInFlightMs('t1')).toBeNull();
  });

  it('goes back to null when the guards are cleared on resume', async () => {
    putThreads([['t1', makeThreadState('t1')]]);
    fetchEvents.mockImplementation(() => new Promise(() => {}));
    void loadThreadEvents('t1');
    await settle();

    clearThreadFetchGuards();

    expect(threadLoadInFlightMs('t1')).toBeNull();
  });
});
