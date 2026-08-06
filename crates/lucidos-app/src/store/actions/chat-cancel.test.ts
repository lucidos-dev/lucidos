/**
 * The cancel action wrapper reports one of three outcomes so the caller
 * (handleCancelExchange) can react correctly:
 *   - 'canceled' — the server canceled live work (a terminal event is coming);
 *     the optimistic `cancelingThreadIds` flag stays until the status transition
 *     releases it.
 *   - 'noop' — the server had nothing to cancel (`{"canceled": false}`); the
 *     client's view is stale, so the flag is released immediately and the thread
 *     re-synced. This is the uncancelable-thread wedge fix: without it, a Stop
 *     click that races a just-finished turn leaves the button disabled forever.
 *   - 'failed' — the API call failed (iOS PWA "Load failed"); the flag rolls
 *     back so the user can retry without reloading.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// refreshThreadEvents is fired on the heal ('noop') path — mock just that
// export (keep the rest of thread-loading real, since the module graph imports
// its other symbols) so the test can assert the re-sync without a real fetch.
const { refreshThreadEvents } = vi.hoisted(() => ({
  refreshThreadEvents: vi.fn(async (_id: string) => {}),
}));
vi.mock('./thread-loading', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./thread-loading')>()),
  refreshThreadEvents,
}));

import { cancelCurrentExchange, handleCancelExchange } from './chat';
import { cancelingThreadIds, focusedThreadId, threadMap } from '../store';
import { setCanceledQuestion, canceledQuestionByThread } from '../../components/chat/prompt-input-helpers';
import type { ThreadMeta, ThreadState } from '../thread-events';

const originalFetch = globalThis.fetch;

function makeThread(channel: ThreadMeta['channel']): ThreadState {
  return {
    meta: {
      id: 't-1',
      title: '',
      channel,
      initiator: 'user',
      saved: false,
      createdAt: '',
      updatedAt: '',
      status: 'running',
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
      lastRevivedAt: '',
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

function setThread(channel: ThreadMeta['channel']) {
  const map = new Map<string, ThreadState>();
  map.set('t-1', makeThread(channel));
  threadMap.value = map;
}

describe('cancelCurrentExchange outcome', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn();
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    focusedThreadId.value = 't-1';
    setThread('chat');
    refreshThreadEvents.mockClear();
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    focusedThreadId.value = null;
    threadMap.value = new Map();
    cancelingThreadIds.value = new Set();
    canceledQuestionByThread.value = new Map();
    vi.restoreAllMocks();
  });

  it("returns 'canceled' when the server reports it canceled work", async () => {
    mockFetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ canceled: true }), { status: 200 }),
    );
    expect(await cancelCurrentExchange()).toBe('canceled');
  });

  it("returns 'canceled' when the body is absent (legacy engine / no JSON)", async () => {
    // An older engine (or any 200 with an empty body) is treated as a real
    // cancel — the pre-existing behavior — so we never spuriously heal.
    mockFetch.mockResolvedValueOnce(new Response(null, { status: 200 }));
    expect(await cancelCurrentExchange()).toBe('canceled');
  });

  it("returns 'noop' when the server reports nothing was canceled", async () => {
    mockFetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ canceled: false }), { status: 200 }),
    );
    expect(await cancelCurrentExchange()).toBe('noop');
  });

  it("returns 'failed' when both retries fail", async () => {
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockRejectedValueOnce(new TypeError('Load failed'));
    expect(await cancelCurrentExchange()).toBe('failed');
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });

  it("returns 'canceled' when the retry succeeds (the iOS PWA case)", async () => {
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce(new Response(JSON.stringify({ canceled: true }), { status: 200 }));
    expect(await cancelCurrentExchange()).toBe('canceled');
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });
});

describe('handleCancelExchange optimistic flag lifecycle', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn();
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    focusedThreadId.value = 't-1';
    setThread('chat');
    cancelingThreadIds.value = new Set();
    canceledQuestionByThread.value = new Map();
    refreshThreadEvents.mockClear();
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    focusedThreadId.value = null;
    threadMap.value = new Map();
    cancelingThreadIds.value = new Set();
    canceledQuestionByThread.value = new Map();
    vi.restoreAllMocks();
  });

  it('keeps the canceling flag on a real cancel (released later by status transition)', async () => {
    mockFetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ canceled: true }), { status: 200 }),
    );
    await handleCancelExchange('t-1');
    expect(cancelingThreadIds.value.has('t-1')).toBe(true);
    expect(refreshThreadEvents).not.toHaveBeenCalled();
  });

  it('heals on a no-op cancel: clears the flag and re-syncs the thread', async () => {
    // The wedge: the server had nothing to cancel (turn already ended), so the
    // optimistic flag must NOT stick — release it and re-sync so the missed
    // terminal event lands and the Cancel button un-disables.
    setCanceledQuestion('t-1', 'q-1');
    mockFetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ canceled: false }), { status: 200 }),
    );
    await handleCancelExchange('t-1');
    expect(cancelingThreadIds.value.has('t-1')).toBe(false);
    expect(canceledQuestionByThread.value.has('t-1')).toBe(false);
    expect(refreshThreadEvents).toHaveBeenCalledWith('t-1');
  });

  it('rolls back the flag on a failed cancel without re-syncing', async () => {
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockRejectedValueOnce(new TypeError('Load failed'));
    await handleCancelExchange('t-1');
    expect(cancelingThreadIds.value.has('t-1')).toBe(false);
    expect(refreshThreadEvents).not.toHaveBeenCalled();
  });
});
