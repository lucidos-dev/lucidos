/**
 * `loadAllThreads` declines a call when another load is in flight, or when the
 * engine is mid-restart. It used to resolve indistinguishably from a landed
 * load, and `refreshThreadList` reads that resolve as "the list is fresh" and
 * retracts its stale-list card: an overlapping resume and SSE resync therefore
 * cleared a warning that was still true, and hid the real load's failure while
 * doing it. Flagged independently by two reviewers on 2026-08-07.
 *
 * It now reports whether THIS call performed a load. These pin both halves: the
 * mutual exclusion (unchanged, and load-bearing so an older response cannot land
 * last and write stale `meta`) and the honest verdict.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
  fetchOlderThreads: vi.fn(),
}));
vi.mock('../../utils/liveness', () => ({ postClientLog: vi.fn() }));

import { fetchThreads } from '../../api/threads';
import type { ThreadsResponse } from '../../api/threads';
import { loadAllThreads } from '../actions/thread-loading';
import { threadMap, focusedThreadId, engineRestarting } from '../store';

const fetchThreadsMock = vi.mocked(fetchThreads);

function emptyResponse(): ThreadsResponse {
  return {
    saved: [],
    archive: [],
    active: [],
    active_threads: [],
    composing: [],
    family_threads: [],
  };
}

/** A response the test settles by hand, so a second caller can arrive while the
 *  first is still open. */
function deferredResponse(): { promise: Promise<ThreadsResponse>; resolve: () => void } {
  let resolve!: () => void;
  const promise = new Promise<ThreadsResponse>((res) => {
    resolve = () => res(emptyResponse());
  });
  return { promise, resolve };
}

beforeEach(() => {
  fetchThreadsMock.mockReset();
  threadMap.value = new Map();
  focusedThreadId.value = null;
  engineRestarting.value = false;
});

describe('loadAllThreads reports whether it actually loaded', () => {
  it('runs one request for overlapping callers, and tells the second it did not load', async () => {
    const gate = deferredResponse();
    fetchThreadsMock.mockReturnValue(gate.promise);

    const first = loadAllThreads();
    // Declines immediately rather than awaiting the open one: WebKit leaves a
    // fetch hanging across an iOS suspension, and joining would block every
    // later caller for the length of it.
    expect(await loadAllThreads()).toBe(false);

    gate.resolve();
    expect(await first).toBe(true);
    expect(fetchThreadsMock).toHaveBeenCalledTimes(1);
  });

  it('starts a fresh load once the previous one has settled', async () => {
    fetchThreadsMock.mockResolvedValue(emptyResponse());

    expect(await loadAllThreads()).toBe(true);
    expect(await loadAllThreads()).toBe(true);
    expect(fetchThreadsMock).toHaveBeenCalledTimes(2);
  });

  it('releases the guard when the load fails, and still reports the failure', async () => {
    fetchThreadsMock.mockRejectedValueOnce(new Error('boom'));
    await expect(loadAllThreads()).rejects.toThrow('boom');

    // A failed load must not wedge the guard: the next attempt has to run.
    fetchThreadsMock.mockResolvedValue(emptyResponse());
    expect(await loadAllThreads()).toBe(true);
  });

  it('reports false without fetching while the engine is restarting', async () => {
    engineRestarting.value = true;
    fetchThreadsMock.mockResolvedValue(emptyResponse());

    expect(await loadAllThreads()).toBe(false);
    expect(fetchThreadsMock).not.toHaveBeenCalled();
  });
});
