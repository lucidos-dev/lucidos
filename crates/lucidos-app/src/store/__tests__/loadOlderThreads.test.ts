import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../api/threads', () => ({
  fetchOlderThreads: vi.fn().mockResolvedValue({ threads: [], family_threads: [], has_more: false }),
  fetchArchivedCount: vi.fn().mockResolvedValue(0),
  fetchThreads: vi.fn().mockResolvedValue({
    saved: [], archive: [], active: [], active_threads: [], composing: [], family_threads: [],
  }),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
}));
vi.mock('../../utils/liveness', () => ({ postClientLog: vi.fn() }));

import { fetchOlderThreads, fetchArchivedCount } from '../../api/threads';
import { loadAllThreads, loadOlderThreads, reloadAfterFilterChange, refreshArchivedCount, filterChangedSinceLoad, _clearFamilyExtensionIdsForTest, _clearLoadedFilterSelectionForTest, _clearOlderThreadsCursorForTest } from '../actions/thread-loading';
import { threadMap, threadHasMore, threadLoadingMore, threadChannelFilter, selectedTriggerIds, selectedRepoIds, selectedAppIds, archiveThreadCount, ALL_CHANNELS } from '../store';
import { closeThreadFilterPanel, openThreadFilterPanel } from '../threadFilterPanel';
import { applySummaryVersion, makeOptimisticThreadState } from '../thread-events';
import type { ThreadState } from '../thread-events';
import type { ThreadSummary } from '../../api/threads';

const fetchMock = vi.mocked(fetchOlderThreads);
const countMock = vi.mocked(fetchArchivedCount);

function summary(over: Partial<ThreadSummary> & { thread_id: string; created_at: string }): ThreadSummary {
  return {
    title: 'Older',
    channel: 'chat',
    initiator: 'user',
    last_activity: over.created_at,
    message_count: 1,
    section: 'archived',
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
    parent_thread_id: null,
    state: 'active',
    compose_text: '',
    compose_images: [],
    ...over,
  };
}

function loaded(thread: ThreadState, updatedAt: string): ThreadState {
  thread.meta.section = 'archived';
  thread.meta.updatedAt = updatedAt;
  thread.meta.createdAt = updatedAt;
  thread.meta.lastUserAction = updatedAt;
  return thread;
}

describe('loadOlderThreads', () => {
  beforeEach(() => {
    fetchMock.mockClear();
    fetchMock.mockResolvedValue({ threads: [], family_threads: [], has_more: false });
    countMock.mockClear();
    countMock.mockResolvedValue(0);
    archiveThreadCount.value = 0;
    threadMap.value = new Map();
    threadHasMore.value = true;
    threadLoadingMore.value = false;
    closeThreadFilterPanel();
    threadChannelFilter.value = new Set(ALL_CHANNELS);
    selectedTriggerIds.value = new Set();
    selectedRepoIds.value = new Set();
    selectedAppIds.value = new Set();
    _clearFamilyExtensionIdsForTest();
    _clearOlderThreadsCursorForTest();
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
      ['coding-agent'],
      undefined,
      ['repo-a'],
      undefined,
    );
  });

  it('pages for the selection ON SCREEN while the filter panel covers the list', async () => {
    threadChannelFilter.value = new Set(['chat']);
    const map = new Map<string, ThreadState>();
    map.set('t1', loaded(makeOptimisticThreadState({
      id: 't1', title: 'T', channel: 'chat', initiator: 'user',
      eventsLoaded: false,
    }), '2026-01-01T00:00:00Z'));
    threadMap.value = map;

    openThreadFilterPanel();
    threadChannelFilter.value = new Set(['claude_code']);
    selectedRepoIds.value = new Set(['repo-a']);

    await loadOlderThreads();

    expect(fetchMock).toHaveBeenCalledWith(
      '2026-01-01T00:00:00Z',
      15,
      ['chat'],
      undefined,
      undefined,
      undefined,
    );

    fetchMock.mockClear();
    threadHasMore.value = true;
    closeThreadFilterPanel();
    await loadOlderThreads();

    expect(fetchMock).toHaveBeenCalledWith(
      expect.any(String),
      15,
      ['coding-agent'],
      undefined,
      ['repo-a'],
      undefined,
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
      undefined,
    );
  });

  it('ignores inbox threads when computing the archive cursor (created_at axis)', async () => {
    const inboxOld = makeOptimisticThreadState({
      id: 'inbox-old', title: 'Long-lived chat', channel: 'chat', initiator: 'user',
      eventsLoaded: false, timestamp: '2026-01-01T00:00:00Z',
    });
    inboxOld.meta.section = 'inbox';
    const archived = makeOptimisticThreadState({
      id: 'arch', title: 'Archived', channel: 'chat', initiator: 'user',
      eventsLoaded: false, timestamp: '2026-06-01T00:00:00Z',
    });
    archived.meta.section = 'archived';
    threadMap.value = new Map([['inbox-old', inboxOld], ['arch', archived]]);

    await loadOlderThreads();

    expect(fetchMock).toHaveBeenLastCalledWith(
      '2026-06-01T00:00:00Z', 15, undefined, undefined, undefined, undefined,
    );
  });

  it.each(['composing', 'discarded'] as const)('ignores an archived %s draft when computing the archive cursor', async (state) => {
    const draft = makeOptimisticThreadState({
      id: 'draft', title: '', channel: 'chat', initiator: 'user',
      eventsLoaded: true, state, status: 'idle', timestamp: '2026-05-05T09:02:09Z',
    });
    draft.meta.section = 'archived';
    draft.meta.createdAt = '2026-05-05T09:02:09Z';
    const archived = loaded(makeOptimisticThreadState({
      id: 'arch', title: 'Archived', channel: 'chat', initiator: 'user', eventsLoaded: false,
    }), '2026-06-09T13:19:05Z');
    archived.meta.section = 'archived';
    threadMap.value = new Map([['draft', draft], ['arch', archived]]);

    await loadOlderThreads();

    expect(fetchMock).toHaveBeenLastCalledWith(
      '2026-06-09T13:19:05Z', 15, undefined, undefined, undefined, undefined,
    );
  });

  it('a failed archived thread participates in the archive cursor (no outlier skipping)', async () => {
    const oldFailed = makeOptimisticThreadState({
      id: 'old-failed', title: 'Old failed', channel: 'chat', initiator: 'user',
      eventsLoaded: false, timestamp: '2026-02-24T00:00:00Z',
    });
    oldFailed.meta.section = 'archived';
    oldFailed.meta.createdAt = '2026-02-24T00:00:00Z';
    applySummaryVersion(oldFailed.meta, oldFailed.meta.summaryVersion, 'failed');
    const recent = loaded(makeOptimisticThreadState({
      id: 'recent', title: 'Recent archived', channel: 'chat', initiator: 'user',
      eventsLoaded: false,
    }), '2026-06-26T00:00:00Z');
    recent.meta.section = 'archived';
    threadMap.value = new Map([['old-failed', oldFailed], ['recent', recent]]);

    await loadOlderThreads();

    expect(fetchMock).toHaveBeenLastCalledWith(
      '2026-02-24T00:00:00Z', 15, undefined, undefined, undefined, undefined,
    );
  });

  it('a family-extension thread does not advance the cursor on the next page', async () => {
    const parent = loaded(makeOptimisticThreadState({
      id: 'parent', title: 'Run nightly', channel: 'trigger', initiator: 'system',
      eventsLoaded: false,
    }), '2026-05-17T01:23:00Z');
    threadMap.value = new Map([['parent', parent]]);

    const oldChildInfo = summary({
      thread_id: 'old-child',
      title: 'Build & test',
      channel: 'claude_code',
      created_at: '2026-05-16T22:46:00Z',
      parent_thread_id: 'parent',
    });
    const sibling: ThreadSummary = {
      ...oldChildInfo,
      thread_id: 'base-1',
      parent_thread_id: null,
      created_at: '2026-05-17T00:30:00Z',
      last_activity: '2026-05-17T00:30:00Z',
      title: 'Some older sibling',
    };
    fetchMock.mockResolvedValueOnce({
      threads: [sibling],
      family_threads: [oldChildInfo],
      has_more: true,
    });
    await loadOlderThreads();
    expect(fetchMock).toHaveBeenLastCalledWith('2026-05-17T01:23:00Z', 15, undefined, undefined, undefined, undefined);

    fetchMock.mockResolvedValueOnce({ threads: [], family_threads: [], has_more: false });
    await loadOlderThreads();
    expect(fetchMock).toHaveBeenLastCalledWith('2026-05-17T00:30:00Z', 15, undefined, undefined, undefined, undefined);
  });

  it('promotes a family-extension thread to base when natural pagination later returns it', async () => {
    const parent = loaded(makeOptimisticThreadState({
      id: 'parent', title: 'P', channel: 'chat', initiator: 'user',
      eventsLoaded: false,
    }), '2026-05-17T01:23:00Z');
    threadMap.value = new Map([['parent', parent]]);

    const oldInfo = summary({
      thread_id: 'old',
      title: 'Old',
      created_at: '2026-05-16T22:46:00Z',
      parent_thread_id: 'parent',
    });
    fetchMock.mockResolvedValueOnce({ threads: [{ ...oldInfo, thread_id: 'mid', last_activity: '2026-05-17T00:30:00Z', parent_thread_id: null }], family_threads: [oldInfo], has_more: true });
    await loadOlderThreads();

    fetchMock.mockResolvedValueOnce({
      threads: [oldInfo, { ...oldInfo, thread_id: 'mid-2', last_activity: '2026-05-16T23:30:00Z', parent_thread_id: null }],
      family_threads: [],
      has_more: false,
    });
    await loadOlderThreads();

    fetchMock.mockResolvedValueOnce({ threads: [], family_threads: [], has_more: false });
    threadHasMore.value = true;
    await loadOlderThreads();
    expect(fetchMock).toHaveBeenLastCalledWith('2026-05-16T22:46:00Z', 15, undefined, undefined, undefined, undefined);
  });

  it('pages BELOW a page that held nothing new, instead of calling the archive exhausted', async () => {
    const arch = loaded(makeOptimisticThreadState({
      id: 'arch', title: 'Oldest loaded archived', channel: 'chat', initiator: 'user',
      eventsLoaded: false,
    }), '2026-06-01T00:00:00Z');
    const alreadyHere = ['inbox-a', 'inbox-b'].map((id, i) => {
      const t = loaded(makeOptimisticThreadState({
        id, title: id, channel: 'chat', initiator: 'user', eventsLoaded: false,
      }), i === 0 ? '2026-05-20T00:00:00Z' : '2026-05-10T00:00:00Z');
      t.meta.section = 'inbox';
      return t;
    });
    threadMap.value = new Map([['arch', arch], ...alreadyHere.map(t => [t.meta.id, t] as const)]);

    fetchMock.mockResolvedValueOnce({
      threads: [
        summary({ thread_id: 'inbox-a', created_at: '2026-05-20T00:00:00Z', section: 'inbox' }),
        summary({ thread_id: 'inbox-b', created_at: '2026-05-10T00:00:00Z', section: 'inbox' }),
      ],
      family_threads: [],
      has_more: true,
    });
    expect(await loadOlderThreads(), 'the server answered, so the page landed').toBe(true);
    expect(fetchMock).toHaveBeenLastCalledWith('2026-06-01T00:00:00Z', 15, undefined, undefined, undefined, undefined);
    expect(threadHasMore.value, 'a page of familiar rows is not the end of the archive').toBe(true);

    fetchMock.mockResolvedValueOnce({ threads: [], family_threads: [], has_more: false });
    await loadOlderThreads();
    expect(fetchMock).toHaveBeenLastCalledWith('2026-05-10T00:00:00Z', 15, undefined, undefined, undefined, undefined);
    expect(threadHasMore.value, 'an empty page IS the end').toBe(false);
  });

  it('reports whether a page landed, so the drawer fill loop knows to ask again', async () => {
    const arch = loaded(makeOptimisticThreadState({
      id: 'arch', title: 'A', channel: 'chat', initiator: 'user', eventsLoaded: false,
    }), '2026-06-01T00:00:00Z');
    threadMap.value = new Map([['arch', arch]]);

    threadLoadingMore.value = true;
    expect(await loadOlderThreads(), 'a declined call fetched nothing').toBe(false);

    threadLoadingMore.value = false;
    fetchMock.mockResolvedValueOnce({
      threads: [summary({ thread_id: 'arch', created_at: '2026-06-01T00:00:00Z' })],
      family_threads: [],
      has_more: true,
    });
    expect(await loadOlderThreads(), 'a landed page always moves the cursor').toBe(true);
  });

  it('keeps a thread inserted while the page was in flight', async () => {
    const seed = loaded(makeOptimisticThreadState({
      id: 'seed', title: 'Seed', channel: 'chat', initiator: 'user',
      eventsLoaded: false,
    }), '2026-01-01T00:00:00Z');
    seed.meta.section = 'archived';
    threadMap.value = new Map([['seed', seed]]);

    let landPage!: (v: { threads: ThreadSummary[]; family_threads: never[]; has_more: boolean }) => void;
    fetchMock.mockReturnValueOnce(new Promise((resolve) => { landPage = resolve; }));
    const inFlight = loadOlderThreads();
    expect(fetchMock, 'the request must be in flight for the race to exist').toHaveBeenCalledTimes(1);

    threadMap.value = new Map([
      ...threadMap.value,
      ['draft', makeOptimisticThreadState({
        id: 'draft', title: '', channel: 'chat', initiator: 'user',
        eventsLoaded: true, state: 'composing', status: 'idle',
      })],
    ]);

    landPage({
      threads: [summary({ thread_id: 'older', created_at: '2025-12-01T00:00:00Z' })],
      family_threads: [],
      has_more: false,
    });
    await inFlight;

    expect(threadMap.value.has('draft'), 'a draft created mid-fetch must survive the page landing').toBe(true);
    expect(threadMap.value.has('older'), 'the fetched page still lands').toBe(true);
  });

  it('reloadAfterFilterChange eagerly fetches matching threads when none are loaded (archived-only repo facet)', async () => {
    threadChannelFilter.value = new Set(['claude_code']);
    selectedRepoIds.value = new Set(['45d9a172-a23e-484b-bf3b-d6a0f9a7983f']);
    threadMap.value = new Map();
    threadHasMore.value = false;

    await reloadAfterFilterChange();

    expect(fetchMock).toHaveBeenCalledOnce();
    const [before, limit, sources, triggerIds, repoIds, appIds] = fetchMock.mock.calls[0];
    expect(sources).toEqual(['coding-agent']);
    expect(repoIds).toEqual(['45d9a172-a23e-484b-bf3b-d6a0f9a7983f']);
    expect(triggerIds).toBeUndefined();
    expect(appIds).toBeUndefined();
    expect(limit).toBe(15);
    expect(typeof before).toBe('string');
  });

  it('falls back to current time when filter matches no loaded thread, so archive is reachable', async () => {
    selectedTriggerIds.value = new Set(['gone-1']);
    threadChannelFilter.value = new Set(['trigger']);
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

  it('refreshArchivedCount fetches the filter-scoped count and stores it', async () => {
    threadChannelFilter.value = new Set(['claude_code']);
    selectedRepoIds.value = new Set(['repo-x']);
    countMock.mockResolvedValueOnce(518);

    await refreshArchivedCount();

    expect(countMock).toHaveBeenCalledOnce();
    const [sources, triggerIds, repoIds, appIds] = countMock.mock.calls[0];
    expect(sources).toEqual(['coding-agent']);
    expect(repoIds).toEqual(['repo-x']);
    expect(triggerIds).toBeUndefined();
    expect(appIds).toBeUndefined();
    expect(archiveThreadCount.value).toBe(518);
  });

  it('refreshArchivedCount counts the whole pile when no filter is active', async () => {
    countMock.mockResolvedValueOnce(4761);

    await refreshArchivedCount();

    const [sources, triggerIds, repoIds, appIds] = countMock.mock.calls[0];
    expect(sources).toBeUndefined();
    expect(triggerIds).toBeUndefined();
    expect(repoIds).toBeUndefined();
    expect(appIds).toBeUndefined();
    expect(archiveThreadCount.value).toBe(4761);
  });

  it('refreshArchivedCount keeps the previous count when the fetch fails (best-effort)', async () => {
    archiveThreadCount.value = 4761;
    countMock.mockRejectedValueOnce(new Error('network'));

    await refreshArchivedCount();

    expect(archiveThreadCount.value).toBe(4761);
  });

  it('refreshArchivedCount reports an empty archive when the channel filter is empty', async () => {
    threadChannelFilter.value = new Set();
    archiveThreadCount.value = 99;

    await refreshArchivedCount();

    expect(countMock).not.toHaveBeenCalled();
    expect(archiveThreadCount.value).toBe(0);
  });
});

describe('filterChangedSinceLoad', () => {
  beforeEach(() => {
    fetchMock.mockClear();
    fetchMock.mockResolvedValue({ threads: [], family_threads: [], has_more: false });
    countMock.mockClear();
    countMock.mockResolvedValue(0);
    threadMap.value = new Map();
    threadHasMore.value = true;
    threadLoadingMore.value = false;
    closeThreadFilterPanel();
    threadChannelFilter.value = new Set(ALL_CHANNELS);
    selectedTriggerIds.value = new Set();
    selectedRepoIds.value = new Set();
    selectedAppIds.value = new Set();
    _clearFamilyExtensionIdsForTest();
    _clearOlderThreadsCursorForTest();
    _clearLoadedFilterSelectionForTest();
  });

  it('is true until a window has been fetched at all', () => {
    expect(filterChangedSinceLoad()).toBe(true);
  });

  it('goes quiet once a reload has fetched against the current selection', async () => {
    await reloadAfterFilterChange();
    expect(filterChangedSinceLoad()).toBe(false);
  });

  it('reports a selection the loaded window was never fetched against', async () => {
    await reloadAfterFilterChange();
    threadChannelFilter.value = new Set(['chat']);
    expect(filterChangedSinceLoad()).toBe(true);
  });

  it('keeps reporting it until the reload actually happens, however often it is asked', async () => {
    await reloadAfterFilterChange();
    selectedRepoIds.value = new Set(['repo-a']);

    expect(filterChangedSinceLoad()).toBe(true);
    expect(filterChangedSinceLoad()).toBe(true);
    expect(filterChangedSinceLoad()).toBe(true);

    await reloadAfterFilterChange();
    expect(filterChangedSinceLoad()).toBe(false);
  });

  it('survives a resync: loadAllThreads must not clear a pending change', async () => {
    await reloadAfterFilterChange();
    selectedRepoIds.value = new Set(['repo-a']);
    expect(filterChangedSinceLoad()).toBe(true);

    await loadAllThreads();

    expect(filterChangedSinceLoad(), 'a resync must not settle a selection it never fetched').toBe(true);
  });

  it('stamps on a cold boot, so the first mount is not a filter change', async () => {
    expect(filterChangedSinceLoad()).toBe(true);
    await loadAllThreads();
    expect(filterChangedSinceLoad()).toBe(false);
  });

  it('does not stamp what a declined call never fetched', async () => {
    threadLoadingMore.value = true;
    selectedRepoIds.value = new Set(['repo-b']);

    await reloadAfterFilterChange();

    expect(fetchMock).not.toHaveBeenCalled();
    expect(filterChangedSinceLoad(), 'a declined fetch settles nothing').toBe(true);
  });

  it('stamps the selection the page was FETCHED for, not whatever is current when it lands', async () => {
    threadChannelFilter.value = new Set(['chat']);
    let landPage!: (v: { threads: never[]; family_threads: never[]; has_more: boolean }) => void;
    fetchMock.mockReturnValueOnce(new Promise((resolve) => { landPage = resolve; }));

    const inFlight = loadOlderThreads();
    expect(fetchMock, 'the request must be in flight for the race to exist').toHaveBeenCalledTimes(1);
    selectedRepoIds.value = new Set(['repo-b']);
    landPage({ threads: [], family_threads: [], has_more: false });
    await inFlight;

    expect(filterChangedSinceLoad(), 'the new selection was never fetched, so it still owes a reload').toBe(true);
    expect(threadHasMore.value, "pagination must stay armed for the selection that was never paged").toBe(true);
  });

  it('notices each facet axis on its own', async () => {
    for (const move of [
      () => { threadChannelFilter.value = new Set(['trigger']); },
      () => { selectedTriggerIds.value = new Set(['trig-a']); },
      () => { selectedRepoIds.value = new Set(['repo-a']); },
      () => { selectedAppIds.value = new Set(['app-a']); },
    ]) {
      await reloadAfterFilterChange();
      expect(filterChangedSinceLoad()).toBe(false);
      move();
      expect(filterChangedSinceLoad()).toBe(true);
    }
  });
});
