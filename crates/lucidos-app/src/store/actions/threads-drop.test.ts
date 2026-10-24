/**
 * A thread list read that went out before a delete must not bring the deleted
 * thread back. While a read is in flight, `loadAllThreads` declines the refresh
 * that `ThreadsDeleted` asks for. The older response is then the last word, and
 * it still lists the thread.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
  fetchOlderThreads: vi.fn(),
}));
vi.mock('../../utils/liveness', () => ({ postClientLog: vi.fn() }));

import { fetchThreads } from '../../api/threads';
import type { ThreadsResponse, ThreadSummary } from '../../api/threads';
import { loadAllThreads } from './thread-loading';
import { dropDeletedThreads } from './threads-drop';
import { threadMap, focusedThreadId, engineRestarting } from '../store';

const fetchThreadsMock = vi.mocked(fetchThreads);

function summary(id: string): ThreadSummary {
  return {
    thread_id: id,
    title: 'a thread',
    channel: 'chat',
    initiator: 'user',
    saved: false,
    section: 'inbox',
    message_count: 1,
    active_children_count: 0,
    total_children_count: 0,
    blocking_descendant_count: 0,
    attention_descendant_count: 0,
    coding_agent_change_state: { kind: 'none' },
    coding_agent_is_external_repo: false,
    last_revived_at: null,
    parent_thread_id: null,
    parent_thread_title: null,
    trigger_id: null,
    trigger_name: null,
    cc_repo_id: null,
    cc_repo_name: null,
    state: 'active',
    compose_text: '',
    compose_images: [],
    compose_mode: null,
    created_at: '2026-05-23T10:00:00Z',
    last_activity: '2026-05-23T10:00:00Z',
    status: 'idle',
  } as unknown as ThreadSummary;
}

function responseListing(...ids: string[]): ThreadsResponse {
  return {
    saved: [],
    archive: [],
    active: [],
    active_threads: ids.map(summary),
    composing: [],
    family_threads: [],
  };
}

beforeEach(() => {
  fetchThreadsMock.mockReset();
  threadMap.value = new Map();
  focusedThreadId.value = null;
  engineRestarting.value = false;
});

describe('a deleted thread stays deleted', () => {
  it('ignores the thread in a list read that went out before the delete', async () => {
    fetchThreadsMock.mockResolvedValueOnce(responseListing('t-gone', 't-kept'));
    await loadAllThreads();
    expect(threadMap.value.has('t-gone')).toBe(true);

    let land!: () => void;
    fetchThreadsMock.mockReturnValueOnce(new Promise<ThreadsResponse>((resolve) => {
      land = () => resolve(responseListing('t-gone', 't-kept'));
    }));
    const inFlight = loadAllThreads();

    dropDeletedThreads(['t-gone']);
    land();
    await inFlight;

    expect(threadMap.value.has('t-gone')).toBe(false);
    expect(threadMap.value.has('t-kept')).toBe(true);
  });
});
