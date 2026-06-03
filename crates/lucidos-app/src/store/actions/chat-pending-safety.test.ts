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
import { PENDING_MESSAGE_SAFETY_MS, STALE_EXCHANGE_FOLLOWUP_MS, clearStalePendingMessages } from './chat';

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
