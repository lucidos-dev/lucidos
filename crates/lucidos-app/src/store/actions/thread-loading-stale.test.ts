import { describe, it, expect, beforeEach, vi } from 'vitest';

// Polyfill localStorage before store.ts is imported at module level.
// vi.hoisted runs before any imports are resolved. Mirrors the scaffolding in
// threads-ensure-status.test.ts, which drives the same real module.
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
  loadThreadEvents,
  markLoadedThreadsStale,
  refreshStaleThreadEvents,
  refreshThreadEvents,
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
  focusIfNeeded: vi.fn(),
  composeHandlers: vi.fn(),
}));

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
}));

const fetchEvents = fetchThreadEvents as unknown as ReturnType<typeof vi.fn>;

/** `refreshThreadEvents` and `loadThreadEvents` are both fire-and-forget from
 *  the sites under test, so let their promise chains settle before asserting. */
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

    // A thread with no events is not BEHIND, it is unloaded, which belongs to
    // `loadThreadEvents` and the failed-load retry rather than to a refresh.
    expect(fetchEvents.mock.calls.map(c => c[0])).toEqual(['has-events']);
  });

  it('rebuilds the set, so a departed thread leaves no entry behind', async () => {
    putThreads([['gone', loaded('gone')], ['stays', loaded('stays')]]);
    markLoadedThreadsStale();

    // Both optimistic-send rollbacks delete a row carrying `eventsLoaded: true`.
    const next = new Map(threadMap.value);
    next.delete('gone');
    threadMap.value = next;
    markLoadedThreadsStale();

    // Re-inserting the id must not find a mark the rebuild should have dropped.
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

    // `runResumeSync` calls this at its top, immediately before marking. If the
    // marks were reset here too, the whole mechanism would silently do nothing.
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
    // Both attempts fail (`refreshThreadEvents` retries a transient rejection
    // once), so nothing landed and the thread is still behind.
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

    // The device slept with the request in flight (WebKit leaves fetches hanging
    // across a suspension) and woke to a new gap. The snapshot about to land
    // predates that gap, so it must not clear the mark it raised.
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

    // A refresh already in flight when the gap opens: a read-after-write heal
    // such as `schedulePendingCleanup`, or a previous focus whose request WebKit
    // left hanging through the suspension. `resyncLoadedThreads` marks without
    // resetting the fetch guards, so its claim is still standing.
    let release!: (v: unknown) => void;
    fetchEvents.mockReturnValueOnce(new Promise(r => { release = r; }));
    void refreshThreadEvents('t1');
    markLoadedThreadsStale();
    refreshStaleThreadEvents('t1');
    await settle();

    // Two requests, not one. Coalescing into the older attempt would spend the
    // open on nothing: it cannot clear the newer mark when it lands, so the
    // thread the user is looking at would stay stale with nothing in flight.
    expect(fetchEvents).toHaveBeenCalledTimes(2);

    release({ events: [], currentAggregate: null });
    await settle();
    refreshStaleThreadEvents('t1');
    await settle();

    // The second request DID cover the gap, so the mark is gone.
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

    // Rapid navigation back onto the same thread must not stack requests: this
    // in-flight attempt WILL clear the mark, so it answers for both callers.
    expect(fetchEvents).toHaveBeenCalledTimes(1);
    release({ events: [], currentAggregate: null });
    await settle();
  });

  it('is consumed by a landed full LOAD, which subsumes a refresh', async () => {
    // A thread marked stale, whose events are then dropped and reloaded from
    // scratch: the snapshot carries no `after`, so it holds everything a refresh
    // would have brought.
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

    // Only the thread the user opened, and incrementally (`?after=lastDbSeq`)
    // rather than as a fresh snapshot.
    expect(fetchEvents.mock.calls).toEqual([['t2', 7]]);
  });

  it('issues no request for a thread that is already current', async () => {
    putThreads([['t1', loaded('t1')]]);

    focusThread('t1');
    await settle();

    expect(fetchEvents).not.toHaveBeenCalled();
  });
});
