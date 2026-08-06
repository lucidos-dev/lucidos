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
      saved: false,
      createdAt: '2026-01-01T00:00:00Z',
      updatedAt: '2026-01-01T00:00:00Z',
      status: 'idle',
      messageCount: 1,
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
    liveEventWaits: [],
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
      event: { type: 'CumulativeTextUpdated', text: 'partial...' },
      created: '2026-04-16T12:00:00Z',
    });

    expect(threadMap.value.has(threadId)).toBe(false);
  });

  it('CodingAgentDiffChanged for unknown thread must NOT create a skeleton', () => {
    const threadId = 'not-loaded-diff-thread';

    handleThreadEvent({
      thread_id: threadId,
      event: { type: 'CodingAgentDiffChanged', has_diff: true },
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

  it('ChildrenCountChanged does not bump meta.updatedAt to the broadcast NOW()', () => {
    // 42aab0773 broadcasts each ancestor's aggregate via ChildrenCountChanged
    // whenever a descendant flips is_blocking. The ancestor isn't doing
    // anything itself — without the targeted exclusion in handleEvent the
    // transient-event branch would set meta.updatedAt = broadcast NOW(),
    // churning the drawer's "X ago" timestamp on every leaf state change
    // across the entire ancestor chain.
    const ancestorId = 'ancestor';
    const ancestorOwnLastActivity = '2026-04-01T00:00:00Z';
    const map = threadMap.value;
    map.set(ancestorId, makeThread(ancestorId, {
      updatedAt: ancestorOwnLastActivity,
      activeChildrenCount: 3,
      totalChildrenCount: 5,
      blockingDescendantCount: 1, attentionDescendantCount: 1,
    }));
    threadMap.value = new Map(map);

    handleThreadEvent({
      thread_id: ancestorId,
      event: { type: 'ChildrenCountChanged', active: 3, total: 5 },
      created: '2026-04-16T12:00:00Z', // broadcast NOW() — must NOT propagate
      aggregate: {
        // Real backend carries the ancestor's OWN unchanged last_activity in
        // the aggregate (update_parent_after_child_terminal doesn't touch
        // last_activity). applyAggregateToMeta overlays it as a no-op.
        threadId: ancestorId,
        title: 'Test Thread',
        channel: 'chat',
        initiator: 'user',
        createdAt: '2026-01-01T00:00:00Z',
        lastActivity: ancestorOwnLastActivity,
        messageCount: 1,
        section: 'archived',
        activeChildrenCount: 3,
        totalChildrenCount: 5,
        blockingDescendantCount: 2, attentionDescendantCount: 2, // descendant flipped → ancestor count moves
        status: 'idle',
        codingAgentHasDiff: false,
        codingAgentProposed: false,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        lastRevivedAt: null,
        isSaved: false,
        hasResponse: true,
        parentThreadId: null,
        parentThreadTitle: null,
        state: 'active',
        latestTodoList: null,
    liveEventWaits: [],
      } as unknown as Parameters<typeof handleThreadEvent>[0]['aggregate'],
    });

    const ancestor = threadMap.value.get(ancestorId)!;
    expect(ancestor.meta.blockingDescendantCount).toBe(2);
    expect(ancestor.meta.updatedAt).toBe(ancestorOwnLastActivity); // NOT bumped
  });

  it('CodingAgentDiffChanged applies aggregate diff flag without bumping updatedAt', () => {
    const threadId = 'coding-agent-thread';
    const ownLastActivity = '2026-04-01T00:00:00Z';
    const map = threadMap.value;
    map.set(threadId, makeThread(threadId, {
      channel: 'claude_code',
      updatedAt: ownLastActivity,
      codingAgentHasDiff: false,
    }));
    threadMap.value = new Map(map);

    handleThreadEvent({
      thread_id: threadId,
      event: { type: 'CodingAgentDiffChanged', has_diff: true },
      created: '2026-04-16T12:00:00Z',
      aggregate: {
        threadId,
        title: 'Test Thread',
        channel: 'claude_code',
        initiator: 'user',
        createdAt: '2026-01-01T00:00:00Z',
        lastActivity: ownLastActivity,
        messageCount: 1,
        section: 'archived',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0, attentionDescendantCount: 0,
        status: 'idle',
        codingAgentHasDiff: true,
        codingAgentProposed: false,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        lastRevivedAt: null,
        isSaved: false,
        hasResponse: true,
        parentThreadId: null,
        parentThreadTitle: null,
        state: 'active',
        latestTodoList: null,
    liveEventWaits: [],
      } as unknown as Parameters<typeof handleThreadEvent>[0]['aggregate'],
    });

    const thread = threadMap.value.get(threadId)!;
    expect(thread.meta.codingAgentHasDiff).toBe(true);
    expect(thread.meta.updatedAt).toBe(ownLastActivity);
  });

  it('ChildrenCountChanged with section flip applies the section change', () => {
    // Regression guard for the broken-short-circuit bug: when a CC child fires
    // ResponseFailed/ResponseCanceled once parent_callback_pending is FALSE, the backend
    // calls update_parent_after_child_terminal(decrement=false, surface_to_inbox=true)
    // — counts stay equal but archive_state flips to 'inbox'. Earlier short-
    // circuit would have dropped the section change. With the cleaner targeted
    // updatedAt exclusion, applyAggregateToMeta still runs and the section moves.
    const parentId = 'parent';
    const map = threadMap.value;
    map.set(parentId, makeThread(parentId, {
      activeChildrenCount: 0,
      totalChildrenCount: 1,
      section: 'archived',
    }));
    threadMap.value = new Map(map);

    handleThreadEvent({
      thread_id: parentId,
      event: { type: 'ChildrenCountChanged', active: 0, total: 1 },
      created: '2026-04-16T12:00:00Z',
      aggregate: {
        threadId: parentId,
        title: 'Test Thread',
        channel: 'chat',
        initiator: 'user',
        createdAt: '2026-01-01T00:00:00Z',
        lastActivity: '2026-04-01T00:00:00Z',
        messageCount: 1,
        section: 'inbox', // <-- surfaced
        activeChildrenCount: 0,
        totalChildrenCount: 1,
        blockingDescendantCount: 0, attentionDescendantCount: 0,
        status: 'idle',
        codingAgentHasDiff: false,
        codingAgentProposed: false,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        lastRevivedAt: null,
        isSaved: false,
        hasResponse: true,
        parentThreadId: null,
        parentThreadTitle: null,
        state: 'active',
        latestTodoList: null,
    liveEventWaits: [],
      } as unknown as Parameters<typeof handleThreadEvent>[0]['aggregate'],
    });

    expect(threadMap.value.get(parentId)!.meta.section).toBe('inbox');
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
