/**
 * Bug: persisted `focusedThreadId` localStorage value can point at a thread
 * the backend doesn't know about (deleted server-side, never committed because
 * the user never sent, or carried over from a different workspace). On reload,
 * ThreadView gates loadThreadEvents on `threadInMap`, so the load never fires
 * and the empty-state spinner spins forever — after 8 seconds the user sees
 * "Taking too long? Tap to reload" and the same id survives every reload
 * because nothing ever clears it.
 *
 * Fix: after loadAllThreads completes its fetch, if `focusedThreadId` is set
 * but the thread is still missing from `threadMap`, clear the persisted id
 * and emit a `cleared_ghost_focus` lifecycle breadcrumb.
 *
 * Observed on an iOS PWA: every Client/lifecycle startup since at least 15:27
 * reported `prev_focused_thread:"5e7139f5-…"` for a thread with zero rows in
 * `events` and no `thread_summaries` entry.
 */
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
import { threadMap, focusedThreadId, FOCUSED_THREAD_KEY } from '../store';

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
    coding_agent_has_diff: false,
    coding_agent_proposed: false,
    coding_agent_requires_restart: false,
    coding_agent_is_external_repo: false,
    coding_agent_applying: false,
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
    try { localStorage.removeItem(FOCUSED_THREAD_KEY); } catch { /* ignore */ }
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
    // Race: ghost id G is captured at the top of loadAllThreadsInner, then the
    // user taps thread R during the await. The clear must check the live
    // signal — not the captured local — before wiping, otherwise it snaps the
    // user back to the compose screen.
    const ghostId = 'ghost-id';
    const userPickedId = 'real-picked-id';
    focusedThreadId.value = ghostId;
    localStorage.setItem(FOCUSED_THREAD_KEY, ghostId);

    const resp = emptyResponse();
    // The real thread the user navigates to mid-await IS in the response — it
    // gets upserted into threadMap normally.
    resp.active_threads = [summary(userPickedId)];
    resp.active = [userPickedId];

    fetchThreadsMock.mockImplementation(async () => {
      // Simulate the user tapping a thread row during the network roundtrip.
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
});
