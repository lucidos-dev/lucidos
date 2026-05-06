/**
 * The cancel action wrapper must return false on failure so the caller
 * (handleCancelExchange) can roll back the optimistic `cancelingThreadIds`
 * flag — otherwise iOS PWA users see "Failed to cancel: Load failed", the
 * Cancel button stays disabled, and they can't retry without reloading.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

import { cancelCurrentExchange } from './chat';
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
      saved: false,
      createdAt: '',
      updatedAt: '',
      status: 'idle',
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      lastRevivedAt: '',
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      state: 'active',
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

describe('cancelCurrentExchange return value', () => {
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
});
