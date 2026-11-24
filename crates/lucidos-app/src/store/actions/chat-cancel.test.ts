import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const { refreshThreadEvents } = vi.hoisted(() => ({
  refreshThreadEvents: vi.fn(async (_id: string) => {}),
}));
vi.mock('./thread-loading', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./thread-loading')>()),
  refreshThreadEvents,
}));

import { cancelCurrentExchange, handleCancelExchange } from './chat';
import { cancelingThreadIds, focusedThreadId, threadMap } from '../store';
import { setCanceledQuestion, canceledQuestionByThread } from '../canceledQuestions';
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
      summaryVersion: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
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
    mockFetch.mockResolvedValueOnce(new Response(null, { status: 200 }));
    expect(await cancelCurrentExchange()).toBe('canceled');
  });

  it("returns 'noop' when the server reports nothing was canceled", async () => {
    mockFetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ canceled: false }), { status: 200 }),
    );
    expect(await cancelCurrentExchange()).toBe('noop');
  });

  it("returns 'failed' on a transport error without replaying a content-bearing Stop", async () => {
    mockFetch.mockRejectedValueOnce(new TypeError('Load failed'));
    expect(await cancelCurrentExchange()).toBe('failed');
    expect(mockFetch).toHaveBeenCalledTimes(1);
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
