/**
 * Bug: Transient SSE events for threads not in the map create phantom skeletons.
 * These "empty threads" show "..." title and "No messages in this thread",
 * then vanish on reload because the API doesn't return them (no DB backing).
 *
 * Root cause: handleThreadEvent creates a skeleton for ANY SSE event, including
 * transient ones (seq=null) like ChildrenCountChanged. Transient events have no
 * DB row, so the skeleton is a phantom.
 *
 * Fix: only create skeletons for persisted events (seq !== null).
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { threadMap, focusedThreadId } from '../store';
import { handleThreadEvent } from '../actions/thread-sync';
import type { ThreadState } from '../thread-events';

// handleThreadEvent uses requestAnimationFrame for batched signal updates.
vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(cb, 0));
vi.stubGlobal('cancelAnimationFrame', (id: number) => clearTimeout(id));

function makeThread(id: string, overrides: Partial<ThreadState['meta']> = {}): ThreadState {
  return {
    meta: {
      id,
      title: 'Test Thread',
      channel: 'chat',
      initiator: 'user',
      pinned: false,
      createdAt: '2026-01-01T00:00:00Z',
      updatedAt: '2026-01-01T00:00:00Z',
      unread: false,
      status: 'idle',
      messageCount: 1,
      section: 'default',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      lastRevivedAt: '',
      ...overrides,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

// ---------------------------------------------------------------------------
// Phantom thread prevention — transient events must not create skeletons
// ---------------------------------------------------------------------------
describe('Phantom thread prevention', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
  });

  it('ChildrenCountChanged for unknown parent must NOT create a skeleton', () => {
    // Scenario: parent chat thread finished long ago, not in recent thread list.
    // Child CC thread starts → backend sends ChildrenCountChanged to parent.
    // Parent is NOT in the map. Without the fix, a phantom skeleton appears.
    const parentId = 'old-parent-thread';

    handleThreadEvent({
      thread_id: parentId,
      // No seq — ChildrenCountChanged is transient
      event: { type: 'ChildrenCountChanged', active: 1, total: 1 },
      created: '2026-04-16T12:00:00Z',
    });

    expect(threadMap.value.has(parentId)).toBe(false);
  });

  it('TextStreaming for unknown thread must NOT create a skeleton', () => {
    // Scenario: SSE connects before loadAllThreads, transient streaming arrives.
    const threadId = 'not-yet-loaded';

    handleThreadEvent({
      thread_id: threadId,
      event: { type: 'TextStreaming', text: 'partial...' },
      created: '2026-04-16T12:00:00Z',
    });

    expect(threadMap.value.has(threadId)).toBe(false);
  });

  it('persisted event for unknown thread DOES create a skeleton', () => {
    // Scenario: new thread starts, SessionStarted arrives via SSE.
    // This should still create a skeleton (it has a DB row).
    const threadId = 'new-thread';

    handleThreadEvent({
      thread_id: threadId,
      seq: 1,
      event: { type: 'SessionStarted', session_id: 'cc-1' },
      created: '2026-04-16T12:00:00Z',
    });

    expect(threadMap.value.has(threadId)).toBe(true);
    const thread = threadMap.value.get(threadId)!;
    expect(thread.meta.title).toBe('...');
    expect(thread.meta.channel).toBe('claude_code');
    expect(thread.eventsLoaded).toBe(false);
  });

  it('ChildrenCountChanged for thread already in map updates it normally', () => {
    // Transient events for threads already in the map should still work.
    const parentId = 'loaded-parent';
    const map = threadMap.value;
    map.set(parentId, makeThread(parentId));
    threadMap.value = new Map(map);

    handleThreadEvent({
      thread_id: parentId,
      event: { type: 'ChildrenCountChanged', active: 2, total: 3 },
      created: '2026-04-16T12:00:00Z',
    });

    const parent = threadMap.value.get(parentId)!;
    expect(parent.meta.activeChildrenCount).toBe(2);
    expect(parent.meta.totalChildrenCount).toBe(3);
  });

  it('CodingAgentThreadSpawned for unknown parent still creates the child thread', () => {
    // CodingAgentThreadSpawned is transient but has a side effect: creating the child CC
    // thread. The parent skeleton should NOT be created, but the child should.
    const parentId = 'old-parent';
    const childId = 'cc-child-new';

    // Focus a different thread (the parent is NOT focused)
    const focusedId = 'my-focused-thread';
    const map = threadMap.value;
    map.set(focusedId, makeThread(focusedId));
    threadMap.value = new Map(map);
    focusedThreadId.value = focusedId;

    handleThreadEvent({
      thread_id: parentId,
      event: { type: 'CodingAgentThreadSpawned', cc_thread_id: childId, title: 'Fix the bug' },
      created: '2026-04-16T12:00:00Z',
    });

    // Parent should NOT be in the map (transient event, not loaded)
    expect(threadMap.value.has(parentId)).toBe(false);
    // Child SHOULD be created by handleTransientSideEffects
    expect(threadMap.value.has(childId)).toBe(true);
    expect(threadMap.value.get(childId)!.meta.title).toBe('Fix the bug');
    expect(threadMap.value.get(childId)!.meta.channel).toBe('claude_code');
  });
});
