import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../api/threads', () => ({
  fetchOlderThreads: vi.fn().mockResolvedValue({ threads: [], has_more: false }),
}));

import { fetchOlderThreads } from '../../api/threads';
import { loadOlderThreads } from '../actions/thread-loading';
import { threadMap, threadHasMore, threadLoadingMore, threadChannelFilter, selectedTriggerIds, selectedRepoIds, ALL_CHANNELS } from '../store';
import { makeOptimisticThreadState } from '../thread-events';
import type { ThreadState } from '../thread-events';

const fetchMock = vi.mocked(fetchOlderThreads);

function loaded(thread: ThreadState, updatedAt: string): ThreadState {
  thread.meta.updatedAt = updatedAt;
  return thread;
}

describe('loadOlderThreads', () => {
  beforeEach(() => {
    fetchMock.mockClear();
    fetchMock.mockResolvedValue({ threads: [], has_more: false });
    threadMap.value = new Map();
    threadHasMore.value = true;
    threadLoadingMore.value = false;
    threadChannelFilter.value = new Set(ALL_CHANNELS);
    selectedTriggerIds.value = new Set();
    selectedRepoIds.value = new Set();
  });

  it('passes selected trigger ids to fetchOlderThreads when set', async () => {
    threadChannelFilter.value = new Set(['trigger']);
    selectedTriggerIds.value = new Set(['trig-a']);
    const map = new Map<string, ThreadState>();
    map.set('t1', loaded(makeOptimisticThreadState({
      id: 't1', title: 'T', channel: 'trigger', initiator: 'system',
      eventsLoaded: false, triggerId: 'trig-a',
    }), '2026-01-01T00:00:00Z'));
    threadMap.value = map;

    await loadOlderThreads();

    expect(fetchMock).toHaveBeenCalledWith(
      '2026-01-01T00:00:00Z',
      15,
      ['trigger'],
      ['trig-a'],
      undefined,
    );
  });

  it('passes selected repo ids to fetchOlderThreads when set', async () => {
    threadChannelFilter.value = new Set(['claude_code']);
    selectedRepoIds.value = new Set(['repo-a']);
    const map = new Map<string, ThreadState>();
    map.set('t1', loaded(makeOptimisticThreadState({
      id: 't1', title: 'T', channel: 'claude_code', initiator: 'user',
      eventsLoaded: false, repoId: 'repo-a',
    }), '2026-01-01T00:00:00Z'));
    threadMap.value = map;

    await loadOlderThreads();

    expect(fetchMock).toHaveBeenCalledWith(
      '2026-01-01T00:00:00Z',
      15,
      ['claude_code'],
      undefined,
      ['repo-a'],
    );
  });

  it('omits trigger_ids when no triggers are selected', async () => {
    threadChannelFilter.value = new Set(['chat']);
    const map = new Map<string, ThreadState>();
    map.set('t1', loaded(makeOptimisticThreadState({
      id: 't1', title: 'T', channel: 'chat', initiator: 'user',
      eventsLoaded: false,
    }), '2026-01-01T00:00:00Z'));
    threadMap.value = map;

    await loadOlderThreads();

    expect(fetchMock).toHaveBeenCalledWith(
      '2026-01-01T00:00:00Z',
      15,
      ['chat'],
      undefined,
      undefined,
    );
  });

  it('falls back to current time when filter matches no loaded thread, so history is reachable', async () => {
    selectedTriggerIds.value = new Set(['gone-1']);
    threadChannelFilter.value = new Set(['trigger']);
    // Loaded threads include a trigger thread for a different trigger
    const map = new Map<string, ThreadState>();
    map.set('t1', loaded(makeOptimisticThreadState({
      id: 't1', title: 'T', channel: 'trigger', initiator: 'system',
      eventsLoaded: false, triggerId: 'other',
    }), '2026-01-01T00:00:00Z'));
    threadMap.value = map;

    await loadOlderThreads();

    expect(fetchMock).toHaveBeenCalledOnce();
    const [before, , sources, triggerIds] = fetchMock.mock.calls[0];
    expect(sources).toEqual(['trigger']);
    expect(triggerIds).toEqual(['gone-1']);
    expect(typeof before).toBe('string');
    expect(before).not.toBe('2026-01-01T00:00:00Z');
  });
});
