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
  it('tells an overlapping caller it did not load, and reloads once for it', async () => {
    const gate = deferredResponse();
    fetchThreadsMock.mockReturnValue(gate.promise);

    const first = loadAllThreads();
    expect(await loadAllThreads()).toBe(false);
    expect(fetchThreadsMock).toHaveBeenCalledTimes(1);

    gate.resolve();
    expect(await first).toBe(true);
    expect(fetchThreadsMock).toHaveBeenCalledTimes(2);
  });

  it('folds every overlapping caller into one reload', async () => {
    const gate = deferredResponse();
    fetchThreadsMock.mockReturnValue(gate.promise);

    const first = loadAllThreads();
    expect(await Promise.all([loadAllThreads(), loadAllThreads()])).toEqual([false, false]);

    gate.resolve();
    await first;
    expect(fetchThreadsMock).toHaveBeenCalledTimes(2);
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
