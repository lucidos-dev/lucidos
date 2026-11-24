import { describe, it, expect, beforeEach, vi } from 'vitest';
import { threadMap, focusedThreadId } from '../store';
import { handleThreadEvent } from '../actions/thread-sync';
import { makeThreadAggregate } from '../actions/threads-test-helpers';
import type { ThreadState } from '../thread-events';

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
      summaryVersion: 0,
      messageCount: 1,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
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

describe('Phantom thread prevention', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
  });

  it('ChildrenCountChanged for unknown parent must NOT create a skeleton', () => {
    const parentId = 'old-parent-thread';

    handleThreadEvent({
      thread_id: parentId,
      event: { type: 'ChildrenCountChanged', active: 1, total: 1 },
      created: '2026-04-16T12:00:00Z',
    });

    expect(threadMap.value.has(parentId)).toBe(false);
  });

  it('TextStreaming for unknown thread must NOT create a skeleton', () => {
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
    const threadId = 'new-thread';

    handleThreadEvent({
      thread_id: threadId,
      seq: 1,
      event: { type: 'SessionStarted', session_id: 'cc-1' },
      created: '2026-04-16T12:00:00Z',
      aggregate: makeThreadAggregate(threadId, { channel: 'claude_code', title: '' }),
    });

    expect(threadMap.value.has(threadId)).toBe(true);
    const thread = threadMap.value.get(threadId)!;
    expect(thread.meta.title).toBe('...');
    expect(thread.meta.channel).toBe('claude_code');
    expect(thread.eventsLoaded).toBe(false);
  });

  it('a skeleton for an old thread takes its identity from the aggregate, not the event', () => {
    const threadId = 'old-archived-cc-thread';

    handleThreadEvent({
      thread_id: threadId,
      seq: 42,
      event: { type: 'WorktreeCleaned', tier: 0, freed_bytes: 1024, branch_deleted: true },
      created: '2026-09-28T06:19:58Z',
      aggregate: makeThreadAggregate(threadId, {
        title: 'Update Icon Alignment and Animation',
        channel: 'claude_code',
        initiator: 'system',
        createdAt: '2026-09-27T19:43:25Z',
        lastActivity: '2026-09-27T20:10:00Z',
        section: 'archived',
      }),
    });

    const meta = threadMap.value.get(threadId)!.meta;
    expect(meta.title).toBe('Update Icon Alignment and Animation');
    expect(meta.channel).toBe('claude_code');
    expect(meta.initiator).toBe('system');
    expect(meta.createdAt).toBe('2026-09-27T19:43:25Z');
    expect(meta.section).toBe('archived');
  });

  it('persisted event for unknown thread with NO aggregate creates nothing', () => {
    const threadId = 'thread-whose-row-was-truncated';

    handleThreadEvent({
      thread_id: threadId,
      seq: 10916,
      event: { type: 'ContextCaptured' },
      created: '2026-04-16T12:00:00Z',
    });

    expect(threadMap.value.has(threadId)).toBe(false);
  });

  it('ChildrenCountChanged for thread already in map updates it normally', () => {
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
      created: '2026-04-16T12:00:00Z',
      aggregate: makeThreadAggregate(ancestorId, {
        lastActivity: ancestorOwnLastActivity,
        section: 'archived',
        activeChildrenCount: 3,
        totalChildrenCount: 5,
        blockingDescendantCount: 2, attentionDescendantCount: 2,
      }),
    });

    const ancestor = threadMap.value.get(ancestorId)!;
    expect(ancestor.meta.blockingDescendantCount).toBe(2);
    expect(ancestor.meta.updatedAt).toBe(ancestorOwnLastActivity);
  });

  it('CodingAgentDiffChanged applies aggregate diff flag without bumping updatedAt', () => {
    const threadId = 'coding-agent-thread';
    const ownLastActivity = '2026-04-01T00:00:00Z';
    const map = threadMap.value;
    map.set(threadId, makeThread(threadId, {
      channel: 'claude_code',
      updatedAt: ownLastActivity,
      codingAgentChangeState: { kind: 'none' },
    }));
    threadMap.value = new Map(map);

    handleThreadEvent({
      thread_id: threadId,
      event: { type: 'CodingAgentDiffChanged', has_diff: true },
      created: '2026-04-16T12:00:00Z',
      aggregate: makeThreadAggregate(threadId, {
        channel: 'claude_code',
        lastActivity: ownLastActivity,
        section: 'archived',
        codingAgentChangeState: { kind: 'unproposed', reason: null },
      }),
    });

    const thread = threadMap.value.get(threadId)!;
    expect(thread.meta.codingAgentChangeState).toEqual({ kind: 'unproposed', reason: null });
    expect(thread.meta.updatedAt).toBe(ownLastActivity);
  });

  it('ChildrenCountChanged with section flip applies the section change', () => {
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
      aggregate: makeThreadAggregate(parentId, {
        lastActivity: '2026-04-01T00:00:00Z',
        section: 'inbox',
        totalChildrenCount: 1,
      }),
    });

    expect(threadMap.value.get(parentId)!.meta.section).toBe('inbox');
  });

  it('CodingAgentThreadSpawned for unknown parent still creates the child thread', () => {
    const parentId = 'old-parent';
    const childId = 'cc-child-new';

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

    expect(threadMap.value.has(parentId)).toBe(false);
    expect(threadMap.value.has(childId)).toBe(true);
    expect(threadMap.value.get(childId)!.meta.title).toBe('Fix the bug');
    expect(threadMap.value.get(childId)!.meta.channel).toBe('claude_code');
  });

  it('CodingAgentThreadSpawned leaves the focused thread\'s pending rows where the user sent them', () => {
    const focusedId = 'thread-the-user-is-typing-in';
    const childId = 'cc-child-of-another-thread';
    const map = threadMap.value;
    const focused = makeThread(focusedId, { initiator: 'user' });
    focused.pendingUserMessages = [{ text: 'my follow-up', eventId: 'evt-mine', created: '2026-04-16T12:00:00Z' }];
    map.set(focusedId, focused);
    threadMap.value = new Map(map);
    focusedThreadId.value = focusedId;

    handleThreadEvent({
      thread_id: childId,
      event: { type: 'CodingAgentThreadSpawned', cc_thread_id: childId, title: 'Fix the bug' },
      created: '2026-04-16T12:00:01Z',
    });

    expect(threadMap.value.get(focusedId)!.pendingUserMessages.map(p => p.eventId)).toEqual(['evt-mine']);
    const child = threadMap.value.get(childId)!;
    expect(child.pendingUserMessages).toEqual([]);
    expect(child.meta.initiator).toBe('system');
  });
});
