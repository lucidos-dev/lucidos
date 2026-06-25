import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../api/threads', () => ({
  fetchOlderThreads: vi.fn().mockResolvedValue({ threads: [], family_threads: [], has_more: false }),
  fetchArchivedCount: vi.fn().mockResolvedValue(0),
}));

import { fetchOlderThreads, fetchArchivedCount } from '../../api/threads';
import { loadOlderThreads, reloadAfterFilterChange, refreshArchivedCount, _clearFamilyExtensionIdsForTest } from '../actions/thread-loading';
import { threadMap, threadHasMore, threadLoadingMore, threadChannelFilter, selectedTriggerIds, selectedRepoIds, selectedAppIds, archiveThreadCount, ALL_CHANNELS } from '../store';
import { makeOptimisticThreadState } from '../thread-events';
import type { ThreadState } from '../thread-events';
import type { ThreadSummary } from '../../api/threads';

const fetchMock = vi.mocked(fetchOlderThreads);
const countMock = vi.mocked(fetchArchivedCount);

function loaded(thread: ThreadState, updatedAt: string): ThreadState {
  thread.meta.updatedAt = updatedAt;
  // The Archive pagination cursor keys on created_at (matching the display
  // sort), so set it to the recency this helper simulates. lastUserAction is set
  // too for the Saved-sort paths that still read it.
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
    threadChannelFilter.value = new Set(ALL_CHANNELS);
    selectedTriggerIds.value = new Set();
    selectedRepoIds.value = new Set();
    selectedAppIds.value = new Set();
    _clearFamilyExtensionIdsForTest();
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
    // Regression: the Archive cursor keys on created_at. An active inbox chat
    // created long ago must NOT drive the cursor — otherwise its old created_at
    // collapses pagination and archived threads created after it are never
    // fetched (the bug a created_at cursor over the whole map would reintroduce,
    // since inbox rows are old-created but recently-active).
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

    // Cursor is the ARCHIVED thread's created_at, not the old inbox thread's.
    expect(fetchMock).toHaveBeenLastCalledWith(
      '2026-06-01T00:00:00Z', 15, undefined, undefined, undefined, undefined,
    );
  });

  it('a family-extension thread does not advance the cursor on the next page', async () => {
    // A trigger parent with a much-older child: the first pagination call
    // returns the parent's sibling (`base-1`) as a base thread plus the
    // older child as a family extension. The next call's cursor must come
    // from the base thread, not the family extension — otherwise infinite
    // scroll skips ~24 h of intermediate threads.
    const parent = loaded(makeOptimisticThreadState({
      id: 'parent', title: 'Run nightly', channel: 'trigger', initiator: 'system',
      eventsLoaded: false,
    }), '2026-05-17T01:23:00Z');
    threadMap.value = new Map([['parent', parent]]);

    const oldChildInfo: ThreadSummary = {
      thread_id: 'old-child',
      title: 'Build & test',
      channel: 'claude_code',
      initiator: 'user',
      created_at: '2026-05-16T22:46:00Z',
      last_activity: '2026-05-16T22:46:00Z',
      message_count: 1,
      section: 'archived',
      active_children_count: 0,
      total_children_count: 0,
      blocking_descendant_count: 0, attention_descendant_count: 0,
      status: 'idle',
      coding_agent_has_diff: false,
      coding_agent_proposed: false,
      coding_agent_requires_restart: false,
      coding_agent_is_external_repo: false,
      coding_agent_applying: false,
      last_revived_at: null,
      parent_thread_id: 'parent',
      state: 'active',
      compose_text: '',
      compose_images: [],
    };
    const sibling: ThreadSummary = {
      ...oldChildInfo,
      thread_id: 'base-1',
      parent_thread_id: null,
      // created_at drives the cursor; distinct from old-child's so the assertion
      // proves the family-extension child (older created_at) is excluded.
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

    // Second call: the family-extension `old-child` is in threadMap but must
    // be skipped by the cursor — otherwise `before` would be 2026-05-16, not
    // the new oldest base thread `base-1`'s 2026-05-17T00:30 timestamp.
    fetchMock.mockResolvedValueOnce({ threads: [], family_threads: [], has_more: false });
    await loadOlderThreads();
    expect(fetchMock).toHaveBeenLastCalledWith('2026-05-17T00:30:00Z', 15, undefined, undefined, undefined, undefined);
  });

  it('promotes a family-extension thread to base when natural pagination later returns it', async () => {
    // First call: server returns nothing base + the old child as family.
    // Second call: natural pagination has reached far enough back to return
    // the old child as base — it must now drive the cursor.
    const parent = loaded(makeOptimisticThreadState({
      id: 'parent', title: 'P', channel: 'chat', initiator: 'user',
      eventsLoaded: false,
    }), '2026-05-17T01:23:00Z');
    threadMap.value = new Map([['parent', parent]]);

    const oldInfo: ThreadSummary = {
      thread_id: 'old',
      title: 'Old',
      channel: 'chat',
      initiator: 'user',
      created_at: '2026-05-16T22:46:00Z',
      last_activity: '2026-05-16T22:46:00Z',
      message_count: 1,
      section: 'archived',
      active_children_count: 0,
      total_children_count: 0,
      blocking_descendant_count: 0, attention_descendant_count: 0,
      status: 'idle',
      coding_agent_has_diff: false,
      coding_agent_proposed: false,
      coding_agent_requires_restart: false,
      coding_agent_is_external_repo: false,
      coding_agent_applying: false,
      last_revived_at: null,
      parent_thread_id: 'parent',
      state: 'active',
      compose_text: '',
      compose_images: [],
    };
    fetchMock.mockResolvedValueOnce({ threads: [{ ...oldInfo, thread_id: 'mid', last_activity: '2026-05-17T00:30:00Z', parent_thread_id: null }], family_threads: [oldInfo], has_more: true });
    await loadOlderThreads();

    // Server returns the old child as BASE — promote it. Use a different
    // base id so `added > 0` and the call doesn't terminate.
    fetchMock.mockResolvedValueOnce({
      threads: [oldInfo, { ...oldInfo, thread_id: 'mid-2', last_activity: '2026-05-16T23:30:00Z', parent_thread_id: null }],
      family_threads: [],
      has_more: false,
    });
    await loadOlderThreads();

    // Third call would cursor off the promoted base thread (oldInfo).
    fetchMock.mockResolvedValueOnce({ threads: [], family_threads: [], has_more: false });
    threadHasMore.value = true; // override the previous has_more=false so we can probe
    await loadOlderThreads();
    expect(fetchMock).toHaveBeenLastCalledWith('2026-05-16T22:46:00Z', 15, undefined, undefined, undefined, undefined);
  });

  it('reloadAfterFilterChange eagerly fetches matching threads when none are loaded (archived-only repo facet)', async () => {
    // Reproduces the reported bug: a repo whose threads are all archived is
    // selected as a filter. None of its threads are in the loaded window, so
    // the drawer renders empty. Selecting the facet MUST deterministically
    // fetch its matches — not wait for the IntersectionObserver sentinel,
    // which is suppressed while Archive is collapsed.
    threadChannelFilter.value = new Set(['claude_code']);
    selectedRepoIds.value = new Set(['45d9a172-a23e-484b-bf3b-d6a0f9a7983f']);
    threadMap.value = new Map(); // nothing loaded matches the facet
    threadHasMore.value = false; // e.g. a prior unfiltered scroll exhausted the list

    await reloadAfterFilterChange();

    // Re-armed pagination so the fetch isn't short-circuited by a stale
    // "no more" from the previous (unfiltered) cursor space.
    expect(fetchMock).toHaveBeenCalledOnce();
    const [before, limit, sources, triggerIds, repoIds, appIds] = fetchMock.mock.calls[0];
    expect(sources).toEqual(['coding-agent']);
    expect(repoIds).toEqual(['45d9a172-a23e-484b-bf3b-d6a0f9a7983f']);
    expect(triggerIds).toBeUndefined();
    expect(appIds).toBeUndefined();
    expect(limit).toBe(15);
    expect(typeof before).toBe('string'); // now()-fallback cursor
  });

  it('falls back to current time when filter matches no loaded thread, so archive is reachable', async () => {
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

  // The Archive badge must "respect the filter": refreshArchivedCount fetches a
  // server-side count scoped to the active channel/facet selection and stores it
  // in archiveThreadCount (the badge reads this directly, so it's stable
  // regardless of how many rows are loaded or whether the section is expanded).
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
    // All channels selected, no facet → undefined params → server counts all.
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
