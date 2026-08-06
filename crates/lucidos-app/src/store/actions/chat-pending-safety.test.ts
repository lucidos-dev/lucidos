/**
 * Tests for the pending message safety timer.
 *
 * Bug: If SSE drops after submitChat() succeeds, the pending user message
 * is never cleared because the MessageReceived SSE event never arrives.
 * effectiveThreadStatus() returns 'running' forever → "Requesting..." stuck.
 *
 * Fix: After submitChat() succeeds, a safety timer calls refreshThreadEvents()
 * after PENDING_MESSAGE_SAFETY_MS. If the pending message is still there after
 * the refresh (e.g., engine down), it's forcefully removed.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { effectiveThreadStatus, threadMap } from '../store';
import type { ThreadState } from '../thread-events';
import { PENDING_MESSAGE_SAFETY_MS, STALE_EXCHANGE_FOLLOWUP_MS, clearStalePendingMessages, schedulePendingCleanup } from './chat';
import { refreshThreadEvents } from './thread-loading';

// Override only refreshThreadEvents; keep the rest of thread-loading real so
// other consumers in chat.ts's import graph are unaffected.
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
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
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

  it('clearStalePendingMessages removes messages older than PENDING_MESSAGE_SAFETY_MS', () => {
    const thread = makeThread();
    const staleTime = new Date(Date.now() - PENDING_MESSAGE_SAFETY_MS - 1000).toISOString();
    thread.pendingUserMessages.push({
      text: 'stale message',
      eventId: 'e-stale',
      created: staleTime,
    });

    const map = new Map([['thread-1', thread]]);
    threadMap.value = map;

    clearStalePendingMessages('thread-1');

    expect(thread.pendingUserMessages).toHaveLength(0);
    expect(effectiveThreadStatus(thread)).toBe('idle');
  });

  it('clearStalePendingMessages keeps recent pending messages', () => {
    const thread = makeThread();
    thread.pendingUserMessages.push({
      text: 'recent message',
      eventId: 'e-recent',
      created: new Date().toISOString(),
    });

    const map = new Map([['thread-1', thread]]);
    threadMap.value = map;

    clearStalePendingMessages('thread-1');

    expect(thread.pendingUserMessages).toHaveLength(1);
    expect(effectiveThreadStatus(thread)).toBe('running');
  });

  it('clearStalePendingMessages removes only stale messages, keeps recent', () => {
    const thread = makeThread();
    const staleTime = new Date(Date.now() - PENDING_MESSAGE_SAFETY_MS - 1000).toISOString();
    thread.pendingUserMessages.push(
      { text: 'stale', eventId: 'e-stale', created: staleTime },
      { text: 'recent', eventId: 'e-recent', created: new Date().toISOString() },
    );

    const map = new Map([['thread-1', thread]]);
    threadMap.value = map;

    clearStalePendingMessages('thread-1');

    expect(thread.pendingUserMessages).toHaveLength(1);
    expect(thread.pendingUserMessages[0].eventId).toBe('e-recent');
  });

  it('PENDING_MESSAGE_SAFETY_MS is exported and is a reasonable value', () => {
    expect(PENDING_MESSAGE_SAFETY_MS).toBeGreaterThanOrEqual(15_000);
    expect(PENDING_MESSAGE_SAFETY_MS).toBeLessThanOrEqual(60_000);
  });

  it('STALE_EXCHANGE_FOLLOWUP_MS is longer than PENDING_MESSAGE_SAFETY_MS', () => {
    expect(STALE_EXCHANGE_FOLLOWUP_MS).toBeGreaterThan(PENDING_MESSAGE_SAFETY_MS);
  });
});

/**
 * The safety cleanup must NOT force-drop a persisted pending message when its
 * recovery refetch failed transiently. `schedulePendingCleanup` only runs after
 * submitChat() succeeded, so the MessageReceived is already in the DB; clearing
 * the optimistic row when the refetch threw (host contention / offline) destroys
 * a message that is safely persisted. That is the `coding-agent-follow-ups:36`
 * "follow-up lost entirely under rapid send-while-working" flake — a follow-up
 * MessageReceived SSE lagging past 30s while the safety refetch also times out.
 * The gating is channel-independent, so a 'chat' thread exercises it cleanly
 * (no CC second-refresh timer to muddy refetch call counts).
 */
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
    // Resolves FALSE, not rejects. `refreshThreadEvents` catches every path, so
    // the rejection this used to mock is a shape it cannot produce: the gate
    // read as always-succeeded in production while looking tested here.
    vi.mocked(refreshThreadEvents).mockResolvedValue(false);
    const thread = makeThread();
    // Old enough that clearStalePendingMessages WOULD drop it — proving the
    // survival is due to the refetch-failure gate, not the recency window.
    const staleTime = new Date(Date.now() - PENDING_MESSAGE_SAFETY_MS - 1000).toISOString();
    thread.pendingUserMessages.push({ text: 'msg2', eventId: 'e-2', created: staleTime });
    threadMap.value = new Map([['thread-1', thread]]);

    schedulePendingCleanup('thread-1', 'e-2');
    await vi.advanceTimersByTimeAsync(PENDING_MESSAGE_SAFETY_MS);

    // Persisted message survives the failed refetch instead of being dropped.
    expect(thread.pendingUserMessages).toHaveLength(1);
    expect(refreshThreadEvents).toHaveBeenCalledTimes(1);

    // A retry was rescheduled — the next window fires another refetch attempt.
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

    // A successful refetch proves the event is absent → safe to drop the stuck row.
    expect(thread.pendingUserMessages).toHaveLength(0);
    expect(refreshThreadEvents).toHaveBeenCalledTimes(1);
  });

  it('stops retrying after the cap but KEEPS the unconfirmed row', async () => {
    vi.mocked(refreshThreadEvents).mockResolvedValue(false);
    const thread = makeThread();
    const staleTime = new Date(Date.now() - PENDING_MESSAGE_SAFETY_MS - 1000).toISOString();
    thread.pendingUserMessages.push({ text: 'msg3', eventId: 'e-3', created: staleTime });
    threadMap.value = new Map([['thread-1', thread]]);

    schedulePendingCleanup('thread-1', 'e-3');
    // Well past the cap: the polling must stop, and the row must survive it.
    await vi.advanceTimersByTimeAsync(PENDING_MESSAGE_SAFETY_MS * 10);

    expect(refreshThreadEvents).toHaveBeenCalledTimes(3);
    // Running out of tries proves no more than one failure did, and dropping
    // here would silently delete a message the engine may well have persisted.
    expect(thread.pendingUserMessages).toHaveLength(1);
    // Marked, so it stops counting as a turn in flight: a bare kept row pins
    // effectiveThreadStatus on 'running' for the life of the page.
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

    // Simulate the real MessageReceived finally landing (SSE recovered).
    thread.pendingUserMessages = [];
    await vi.advanceTimersByTimeAsync(PENDING_MESSAGE_SAFETY_MS);

    // The rescheduled timer exits at the guard — no further refetch churn.
    expect(refreshThreadEvents).toHaveBeenCalledTimes(1);
  });
});
