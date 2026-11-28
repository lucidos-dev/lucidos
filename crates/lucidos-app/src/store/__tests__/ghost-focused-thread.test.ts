import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
  fetchOlderThreads: vi.fn(),
}));

vi.mock('../../utils/liveness', () => ({
  postClientLog: vi.fn(),
}));

import { fetchThreads, fetchThreadEvents } from '../../api/threads';
import type { ThreadsResponse, ThreadSummary } from '../../api/threads';
import { postClientLog } from '../../utils/liveness';
import { loadAllThreads } from '../actions/thread-loading';
import { threadMap, awaitedThreadId, focusedThreadId, FOCUSED_THREAD_KEY } from '../store';

const fetchThreadsMock = vi.mocked(fetchThreads);
const fetchEventsMock = vi.mocked(fetchThreadEvents);
const postClientLogMock = vi.mocked(postClientLog);

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

function summary(id: string): ThreadSummary {
  return {
    thread_id: id,
    title: 'real',
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

describe('loadAllThreads — ghost focused-thread clear', () => {
  beforeEach(() => {
    fetchThreadsMock.mockReset();
    fetchEventsMock.mockClear();
    postClientLogMock.mockClear();
    threadMap.value = new Map();
    focusedThreadId.value = null;
    awaitedThreadId.value = null;
    try { localStorage.removeItem(FOCUSED_THREAD_KEY); } catch {  }
  });

  it('clears focusedThreadId + localStorage when backend has no record of it', async () => {
    const ghostId = 'ghost-thread-id';
    focusedThreadId.value = ghostId;
    localStorage.setItem(FOCUSED_THREAD_KEY, ghostId);
    fetchThreadsMock.mockResolvedValue(emptyResponse());

    await loadAllThreads();

    expect(focusedThreadId.value).toBeNull();
    expect(localStorage.getItem(FOCUSED_THREAD_KEY)).toBeNull();
    expect(threadMap.value.has(ghostId)).toBe(false);
    expect(fetchEventsMock).not.toHaveBeenCalledWith(ghostId);
    expect(postClientLogMock).toHaveBeenCalledWith(
      'lifecycle',
      'cleared_ghost_focus',
      { thread_id: ghostId },
    );
  });

  it('keeps focusedThreadId when backend returns it via focused_thread', async () => {
    const realId = 'real-thread-id';
    focusedThreadId.value = realId;
    localStorage.setItem(FOCUSED_THREAD_KEY, realId);
    const resp = emptyResponse();
    resp.focused_thread = summary(realId);
    fetchThreadsMock.mockResolvedValue(resp);

    await loadAllThreads();

    expect(focusedThreadId.value).toBe(realId);
    expect(localStorage.getItem(FOCUSED_THREAD_KEY)).toBe(realId);
    expect(threadMap.value.has(realId)).toBe(true);
    expect(postClientLogMock).not.toHaveBeenCalledWith(
      'lifecycle',
      'cleared_ghost_focus',
      expect.anything(),
    );
  });

  it('keeps focusedThreadId when the thread is in active_threads', async () => {
    const realId = 'real-thread-id';
    focusedThreadId.value = realId;
    localStorage.setItem(FOCUSED_THREAD_KEY, realId);
    const resp = emptyResponse();
    resp.active_threads = [summary(realId)];
    resp.active = [realId];
    fetchThreadsMock.mockResolvedValue(resp);

    await loadAllThreads();

    expect(focusedThreadId.value).toBe(realId);
    expect(threadMap.value.has(realId)).toBe(true);
    expect(postClientLogMock).not.toHaveBeenCalledWith(
      'lifecycle',
      'cleared_ghost_focus',
      expect.anything(),
    );
  });

  it('no-op when focusedThreadId was already null', async () => {
    fetchThreadsMock.mockResolvedValue(emptyResponse());

    await loadAllThreads();

    expect(focusedThreadId.value).toBeNull();
    expect(postClientLogMock).not.toHaveBeenCalled();
  });

  it('does not wipe a user navigation that landed during the fetchThreads await', async () => {
    const ghostId = 'ghost-id';
    const userPickedId = 'real-picked-id';
    focusedThreadId.value = ghostId;
    localStorage.setItem(FOCUSED_THREAD_KEY, ghostId);

    const resp = emptyResponse();
    resp.active_threads = [summary(userPickedId)];
    resp.active = [userPickedId];

    fetchThreadsMock.mockImplementation(async () => {
      focusedThreadId.value = userPickedId;
      localStorage.setItem(FOCUSED_THREAD_KEY, userPickedId);
      return resp;
    });

    await loadAllThreads();

    expect(focusedThreadId.value).toBe(userPickedId);
    expect(localStorage.getItem(FOCUSED_THREAD_KEY)).toBe(userPickedId);
    expect(postClientLogMock).not.toHaveBeenCalledWith(
      'lifecycle',
      'cleared_ghost_focus',
      expect.anything(),
    );
  });

  it('keeps an awaited thread the backend has not heard of yet', async () => {
    const spawnedId = 'queued-setup-thread';
    awaitedThreadId.value = spawnedId;
    focusedThreadId.value = spawnedId;
    localStorage.setItem(FOCUSED_THREAD_KEY, spawnedId);
    fetchThreadsMock.mockResolvedValue(emptyResponse());

    await loadAllThreads();

    expect(focusedThreadId.value).toBe(spawnedId);
    expect(postClientLogMock).not.toHaveBeenCalledWith(
      'lifecycle',
      'cleared_ghost_focus',
      expect.anything(),
    );
  });
});
