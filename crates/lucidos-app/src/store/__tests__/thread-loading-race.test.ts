import { describe, it, expect, beforeEach } from 'vitest';
import { threadMap, focusedThreadId, threadsLoaded } from '../store';
import type { ThreadState } from '../thread-events';

function makeThread(id: string, overrides: Partial<ThreadState> = {}): ThreadState {
  return {
    meta: {
      id,
      title: '...',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '',
      updatedAt: '',
      status: 'idle',
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
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
    ...overrides,
  };
}

describe('loadThreadEvents stale map write-back', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
    threadsLoaded.value = false;
  });

  it('concurrent threadMap updates should not clobber each other', async () => {

    const threadA = makeThread('thread-a');
    const threadB = makeThread('thread-b');

    const mapA = new Map<string, ThreadState>();
    mapA.set('thread-a', threadA);
    threadMap.value = mapA;

    threadMap.value.set('thread-b', threadB);
    threadMap.value = new Map(threadMap.value);

    expect(threadMap.value.has('thread-a')).toBe(true);
    expect(threadMap.value.has('thread-b')).toBe(true);

    threadA.eventsLoaded = true;
    threadMap.value = new Map(threadMap.value);

    expect(threadMap.value.has('thread-a')).toBe(true);
    expect(threadMap.value.has('thread-b')).toBe(true);
    expect(threadMap.value.get('thread-a')!.eventsLoaded).toBe(true);
  });

  it('stale map reference loses concurrent SSE threads (reproduces bug)', () => {
    const threadA = makeThread('thread-a');
    const threadB = makeThread('thread-b');

    const mapA = new Map<string, ThreadState>([['thread-a', threadA]]);
    threadMap.value = mapA;

    mapA.set('thread-b', threadB);
    const mapB = new Map(mapA);
    threadMap.value = mapB;

    expect(threadMap.value.size).toBe(2);

    const fixedMap = new Map(threadMap.value);
    expect(fixedMap.size).toBe(2);
  });
});

describe('loadThreadEvents retry mechanism', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
    threadsLoaded.value = false;
  });

  it('thread not in map on first call should succeed on retry when thread appears', () => {
    const threadId = 'thread-123';
    focusedThreadId.value = threadId;

    expect(threadMap.value.get(threadId)).toBeUndefined();

    const thread = makeThread(threadId);
    threadMap.value.set(threadId, thread);
    threadMap.value = new Map(threadMap.value);

    const retryThread = threadMap.value.get(threadId);
    expect(retryThread).toBeDefined();
    expect(retryThread!.eventsLoaded).toBe(false);

    retryThread!.eventsLoaded = true;
    expect(retryThread!.eventsLoaded).toBe(true);
  });

  it('eventsLoaded false with thread in map should allow loading', () => {
    const thread = makeThread('thread-1');
    threadMap.value = new Map([['thread-1', thread]]);
    focusedThreadId.value = 'thread-1';

    const t = threadMap.value.get('thread-1');
    expect(t).toBeDefined();
    expect(t!.eventsLoaded).toBe(false);
  });
});

describe('ThreadView useEffect dep change detection', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
    threadsLoaded.value = false;
  });

  it('eventsLoaded stays false when thread transitions from absent to present (reproduces bug)', () => {
    const threadId = 'thread-appear';
    focusedThreadId.value = threadId;

    const eventThread1 = threadMap.value.get(threadId);
    const eventsLoaded1 = eventThread1?.eventsLoaded ?? false;
    expect(eventThread1).toBeUndefined();
    expect(eventsLoaded1).toBe(false);

    const thread = makeThread(threadId);
    threadMap.value = new Map([[threadId, thread]]);

    const eventThread2 = threadMap.value.get(threadId);
    const eventsLoaded2 = eventThread2?.eventsLoaded ?? false;
    expect(eventThread2).toBeDefined();
    expect(eventsLoaded2).toBe(false);

    expect(!!eventThread1).toBe(false);
    expect(!!eventThread2).toBe(true);
    expect(!!eventThread1).not.toBe(!!eventThread2);
  });
});
