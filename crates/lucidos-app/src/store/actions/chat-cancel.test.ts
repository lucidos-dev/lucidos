/**
 * The cancel/interrupt action wrappers must return false on failure so the
 * caller can roll back the optimistic `canceling` UI flag — otherwise iOS PWA
 * users see "Failed to interrupt: Load failed", the stop button vanishes
 * (because `canceling` stayed true), and they can't retry without reloading.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

import { cancelCurrentExchange, interruptCurrentExchange } from './chat';
import { focusedThreadId, threadMap } from '../store';
import type { ThreadMeta, ThreadState } from '../thread-events';

const originalFetch = globalThis.fetch;

function makeThread(channel: ThreadMeta['channel']): ThreadState {
  return {
    meta: {
      id: 't-1',
      title: '',
      channel,
      initiator: 'user',
      pinned: false,
      createdAt: '',
      updatedAt: '',
      unread: false,
      status: 'idle',
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      lastRevivedAt: '',
      messageCount: 0,
      section: 'default',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
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

describe('cancelCurrentExchange / interruptCurrentExchange return value', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn();
    globalThis.fetch = mockFetch as unknown as typeof fetch;
    focusedThreadId.value = 't-1';
    setThread('chat');
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    focusedThreadId.value = null;
    threadMap.value = new Map();
    vi.restoreAllMocks();
  });

  it('cancelCurrentExchange returns true when cancel API succeeds', async () => {
    mockFetch.mockResolvedValueOnce(new Response(null, { status: 200 }));
    const ok = await cancelCurrentExchange();
    expect(ok).toBe(true);
  });

  it('cancelCurrentExchange returns false when both retries fail', async () => {
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockRejectedValueOnce(new TypeError('Load failed'));
    const ok = await cancelCurrentExchange();
    expect(ok).toBe(false);
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });

  it('cancelCurrentExchange returns true when retry succeeds (the iOS PWA case)', async () => {
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce(new Response(null, { status: 200 }));
    const ok = await cancelCurrentExchange();
    expect(ok).toBe(true);
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });

  it('interruptCurrentExchange returns true when interrupt API succeeds', async () => {
    setThread('claude_code');
    mockFetch.mockResolvedValueOnce(new Response(null, { status: 200 }));
    const ok = await interruptCurrentExchange();
    expect(ok).toBe(true);
  });

  it('interruptCurrentExchange returns false when both retries fail', async () => {
    setThread('claude_code');
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockRejectedValueOnce(new TypeError('Load failed'));
    const ok = await interruptCurrentExchange();
    expect(ok).toBe(false);
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });

  it('interruptCurrentExchange returns true when retry succeeds (the iOS PWA case)', async () => {
    setThread('claude_code');
    mockFetch
      .mockRejectedValueOnce(new TypeError('Load failed'))
      .mockResolvedValueOnce(new Response(null, { status: 200 }));
    const ok = await interruptCurrentExchange();
    expect(ok).toBe(true);
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });
});
