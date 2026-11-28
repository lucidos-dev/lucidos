import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { effectiveThreadStatus, threadMap } from '../store';
import type { ThreadState } from '../thread-events';
import { PENDING_MESSAGE_SAFETY_MS, STALE_EXCHANGE_FOLLOWUP_MS, schedulePendingCleanup } from './chat';
import { refreshThreadEvents } from './thread-loading';

vi.mock('./thread-loading', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./thread-loading')>();
  return { ...actual, refreshThreadEvents: vi.fn().mockResolvedValue(true) };
});

function makeThread(id = 'thread-1'): ThreadState {
  return {
    meta: {
      id,
      title: 'Test',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '',
      updatedAt: '',
      status: 'idle',
      summaryVersion: 0,
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0, archivableSubThreadCount: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
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

describe('Pending message safety timer', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    threadMap.value = new Map();
  });

  it('effectiveThreadStatus returns running when pending messages exist', () => {
    const thread = makeThread();
    thread.pendingUserMessages.push({
      text: 'hello',
      eventId: 'e-1',
      created: new Date().toISOString(),
    });
    expect(effectiveThreadStatus(thread)).toBe('running');
  });

  it('effectiveThreadStatus returns idle when no pending messages', () => {
    const thread = makeThread();
    expect(effectiveThreadStatus(thread)).toBe('idle');
  });

  it('PENDING_MESSAGE_SAFETY_MS is exported and is a reasonable value', () => {
    expect(PENDING_MESSAGE_SAFETY_MS).toBeGreaterThanOrEqual(15_000);
    expect(PENDING_MESSAGE_SAFETY_MS).toBeLessThanOrEqual(60_000);
  });

  it('STALE_EXCHANGE_FOLLOWUP_MS is longer than PENDING_MESSAGE_SAFETY_MS', () => {
    expect(STALE_EXCHANGE_FOLLOWUP_MS).toBeGreaterThan(PENDING_MESSAGE_SAFETY_MS);
  });
});

describe('schedulePendingCleanup gates force-clear on refetch success', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(refreshThreadEvents).mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
    threadMap.value = new Map();
  });

  it('keeps a stale pending message when the safety refetch FAILS, and retries', async () => {
    vi.mocked(refreshThreadEvents).mockResolvedValue(false);
    const thread = makeThread();
    const staleTime = new Date(Date.now() - PENDING_MESSAGE_SAFETY_MS - 1000).toISOString();
    thread.pendingUserMessages.push({ text: 'msg2', eventId: 'e-2', created: staleTime });
    threadMap.value = new Map([['thread-1', thread]]);

    schedulePendingCleanup('thread-1', 'e-2');
    await vi.advanceTimersByTimeAsync(PENDING_MESSAGE_SAFETY_MS);

    expect(thread.pendingUserMessages).toHaveLength(1);
    expect(refreshThreadEvents).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(PENDING_MESSAGE_SAFETY_MS);
    expect(refreshThreadEvents).toHaveBeenCalledTimes(2);
    expect(thread.pendingUserMessages).toHaveLength(1);
  });

  it('force-clears a stale pending message when the safety refetch SUCCEEDS but the event is genuinely absent', async () => {
    vi.mocked(refreshThreadEvents).mockResolvedValue(true);
    const thread = makeThread();
    const staleTime = new Date(Date.now() - PENDING_MESSAGE_SAFETY_MS - 1000).toISOString();
    thread.pendingUserMessages.push({ text: 'never-persisted', eventId: 'e-gone', created: staleTime });
    threadMap.value = new Map([['thread-1', thread]]);

    schedulePendingCleanup('thread-1', 'e-gone');
    await vi.advanceTimersByTimeAsync(PENDING_MESSAGE_SAFETY_MS);

    expect(thread.pendingUserMessages).toHaveLength(0);
    expect(refreshThreadEvents).toHaveBeenCalledTimes(1);
  });

  it('removes only its own send\'s row, never one that is still sending', async () => {
    vi.mocked(refreshThreadEvents).mockResolvedValue(true);
    const thread = makeThread();
    const staleTime = new Date(Date.now() - PENDING_MESSAGE_SAFETY_MS - 1000).toISOString();
    thread.pendingUserMessages.push(
      { text: 'accepted', eventId: 'e-accepted', created: staleTime },
      { text: 'still retrying', eventId: 'e-retrying', created: staleTime },
    );
    threadMap.value = new Map([['thread-1', thread]]);

    schedulePendingCleanup('thread-1', 'e-accepted');
    await vi.advanceTimersByTimeAsync(PENDING_MESSAGE_SAFETY_MS);

    expect(thread.pendingUserMessages.map(p => p.eventId)).toEqual(['e-retrying']);
  });

  it('stops retrying after the cap but KEEPS the unconfirmed row', async () => {
    vi.mocked(refreshThreadEvents).mockResolvedValue(false);
    const thread = makeThread();
    const staleTime = new Date(Date.now() - PENDING_MESSAGE_SAFETY_MS - 1000).toISOString();
    thread.pendingUserMessages.push({ text: 'msg3', eventId: 'e-3', created: staleTime });
    threadMap.value = new Map([['thread-1', thread]]);

    schedulePendingCleanup('thread-1', 'e-3');
    await vi.advanceTimersByTimeAsync(PENDING_MESSAGE_SAFETY_MS * 10);

    expect(refreshThreadEvents).toHaveBeenCalledTimes(3);
    expect(thread.pendingUserMessages).toHaveLength(1);
    expect(thread.pendingUserMessages[0].unconfirmed).toBe(true);
    expect(effectiveThreadStatus(thread)).not.toBe('running');
  });

  it('stops retrying once the pending message is gone (SSE caught up)', async () => {
    vi.mocked(refreshThreadEvents).mockResolvedValue(false);
    const thread = makeThread();
    thread.pendingUserMessages.push({ text: 'msg2', eventId: 'e-2', created: new Date().toISOString() });
    threadMap.value = new Map([['thread-1', thread]]);

    schedulePendingCleanup('thread-1', 'e-2');
    await vi.advanceTimersByTimeAsync(PENDING_MESSAGE_SAFETY_MS);
    expect(refreshThreadEvents).toHaveBeenCalledTimes(1);

    thread.pendingUserMessages = [];
    await vi.advanceTimersByTimeAsync(PENDING_MESSAGE_SAFETY_MS);

    expect(refreshThreadEvents).toHaveBeenCalledTimes(1);
  });
});
