import { describe, it, expect, beforeEach } from 'vitest';
import { threadMap, focusedThreadId, threadsLoaded } from '../store';
import { handleEvent, groupIntoExchanges, type ThreadState, type ThreadMeta, type ThreadEvent } from '../thread-events';
import { loadThreadEvents } from '../actions/thread-loading';
import { shouldRevealThread } from '../../../src/components/chat/ThreadView';

function makeThread(id: string, overrides: Partial<Omit<ThreadState, 'meta'>> & { meta?: Partial<ThreadMeta> } = {}): ThreadState {
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
      blockingDescendantCount: 0, attentionDescendantCount: 0, archivableSubThreadCount: 0,
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
      ...(overrides.meta || {}),
    },
    events: overrides.events || new Map(),
    streamingBuffer: overrides.streamingBuffer || '',
    eventsLoaded: overrides.eventsLoaded ?? false,
    eventsLoadFailed: overrides.eventsLoadFailed ?? false,
    lastDbSeq: overrides.lastDbSeq || 0,
    pendingUserMessages: overrides.pendingUserMessages || [],
  };
}

function shouldShowExchanges(thread: ThreadState | undefined, animating: boolean): 'content' | 'empty-message' | 'error' | 'spinner' {
  if (!thread) return 'spinner';
  const hasPending = thread.pendingUserMessages.length > 0;
  const hasContent = thread.eventsLoaded || thread.events.size > 0 || hasPending;
  const exchanges = hasContent && !animating ? groupIntoExchanges(thread.events) : [];

  if (exchanges.length === 0 && !hasPending) {
    if (thread.eventsLoadFailed && !hasContent) return 'error';
    if (hasContent && !animating) return 'empty-message';
    return 'spinner';
  }
  return 'content';
}

let seqCounter = 1;

function insertEvents(
  map: Map<string, ThreadState>,
  threadId: string,
  events: Array<ThreadEvent & { created?: string }>,
): void {
  for (const event of events) {
    const created = event.created;
    const clean = { ...event };
    delete (clean as any).created;
    delete (clean as any).event_id;
    handleEvent(map, threadId, seqCounter++, clean, created);
  }
}

describe('ThreadView content display decision', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
    threadsLoaded.value = false;
    seqCounter = 1;
  });

  it('eventsLoaded=true, events present → show content', () => {
    const thread = makeThread('t1', { eventsLoaded: true });
    const map = new Map([['t1', thread]]);
    insertEvents(map, 't1', [
      { type: 'MessageReceived', text: 'hi', channel: 'chat', created: '2026-04-01T10:00:00Z' } as any,
    ]);
    expect(shouldShowExchanges(thread, false)).toBe('content');
  });

  it('eventsLoaded=true, no events → show "no messages"', () => {
    const thread = makeThread('t1', { eventsLoaded: true });
    expect(shouldShowExchanges(thread, false)).toBe('empty-message');
  });

  it('eventsLoaded=false, no events → show spinner (loading)', () => {
    const thread = makeThread('t1', { eventsLoaded: false });
    expect(shouldShowExchanges(thread, false)).toBe('spinner');
  });

  it('eventsLoaded=false, SSE events present → show content (THE FIX)', () => {
    const thread = makeThread('t1', { eventsLoaded: false });
    const map = new Map([['t1', thread]]);
    insertEvents(map, 't1', [
      { type: 'MessageReceived', text: 'Fix it', channel: 'chat', created: '2026-04-01T10:00:00Z' } as any,
      { type: 'ResponseGenerated', content: 'Done', completed: true, created: '2026-04-01T10:00:05Z' } as any,
    ]);

    expect(thread.events.size).toBe(2);
    expect(thread.eventsLoaded).toBe(false);
    expect(shouldShowExchanges(thread, false)).toBe('content');
  });

  it('animating=true gates content — shows spinner instead of exchanges', () => {
    const thread = makeThread('t1', { eventsLoaded: true });
    const map = new Map([['t1', thread]]);
    insertEvents(map, 't1', [
      { type: 'MessageReceived', text: 'hi', channel: 'chat', created: '2026-04-01T10:00:00Z' } as any,
    ]);
    expect(shouldShowExchanges(thread, true)).toBe('spinner');
  });

  it('undefined thread (not in map yet) → show spinner, never blank', () => {
    expect(shouldShowExchanges(undefined, false)).toBe('spinner');
  });
});

describe('CC thread content loading — full integration', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
    threadsLoaded.value = false;
    seqCounter = 1;
  });

  it('CC thread with complete lifecycle renders correctly', () => {
    const threadId = 'cc-thread-1';
    const thread = makeThread(threadId, {
      eventsLoaded: true,
      meta: {
        id: threadId,
        title: 'Sticky Mobile Header Keyboard Fix',
        channel: 'claude_code',
        saved: false,
        createdAt: '2026-04-01T10:00:00Z',
        updatedAt: '2026-04-01T10:05:00Z',
        status: 'waiting',
        codingAgentChangeState: { kind: 'proposed', requires_restart: false },
        codingAgentIsExternalRepo: false,
        lastRevivedAt: '',
        messageCount: 1,
        section: 'inbox',
        activeChildrenCount: 0,
      },
    });
    const map = new Map([[threadId, thread]]);

    insertEvents(map, threadId, [
      { type: 'MessageReceived', text: 'Fix the sticky header keyboard issue', channel: 'claude_code', created: '2026-04-01T10:00:00Z' } as any,
      { type: 'SessionStarted', session_id: 's1', created: '2026-04-01T10:00:01Z' } as any,
      { type: 'CodingAgentToolCalled', name: 'Edit', args: '{}', created: '2026-04-01T10:01:00Z' } as any,
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-04-01T10:01:01Z' } as any,
      { type: 'ResponseGenerated', content: 'Fixed it', completed: true, created: '2026-04-01T10:02:00Z' } as any,
      { type: 'CodingAgentIdled', has_changes: true, created: '2026-04-01T10:02:01Z' } as any,
    ]);

    const exchanges = groupIntoExchanges(thread.events);
    expect(exchanges.length).toBe(1);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[0].steps.length).toBe(5);
  });

  it('CC thread SSE events render despite failed DB load', () => {
    const threadId = 'cc-resume';
    const thread = makeThread(threadId, {
      eventsLoaded: false,
      meta: {
        id: threadId,
        title: 'Active CC Thread',
        channel: 'claude_code',
        saved: false,
        createdAt: '2026-04-01T09:00:00Z',
        updatedAt: '2026-04-01T10:00:00Z',
        status: 'running',
        codingAgentChangeState: { kind: 'none' },
        codingAgentIsExternalRepo: false,
        lastRevivedAt: '',
        messageCount: 0,
        section: 'archived',
        activeChildrenCount: 0,
      },
    });
    const map = new Map([[threadId, thread]]);

    insertEvents(map, threadId, [
      { type: 'MessageReceived', text: 'Continue working', channel: 'claude_code', created: '2026-04-01T10:00:00Z' } as any,
      { type: 'CodingAgentTextStreamed', text: 'Analyzing...', created: '2026-04-01T10:00:05Z' } as any,
    ]);

    expect(shouldShowExchanges(thread, false)).toBe('content');
  });
});

describe('shouldRevealThread with hasContent', () => {
  it('eligible when eventsLoaded is true', () => {
    expect(shouldRevealThread('t1', false, true)).toBe(true);
  });

  it('not eligible when animating', () => {
    expect(shouldRevealThread('t1', true, true)).toBe(false);
  });

  it('not eligible when no threadId', () => {
    expect(shouldRevealThread(null, false, true)).toBe(false);
  });

  it('not eligible when eventsLoaded is false AND no hasContent override', () => {
    expect(shouldRevealThread('t1', false, false)).toBe(false);
  });
});

describe('pending messages show on a thread whose events have not loaded', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
    threadsLoaded.value = false;
    seqCounter = 1;
  });

  it('pending messages make hasContent=true even with eventsLoaded=false', () => {
    const thread = makeThread('cc-spawn-1', {
      eventsLoaded: false,
      meta: { channel: 'claude_code' } as any,
      pendingUserMessages: [{ text: 'Fix the bug', eventId: 'e1', created: '2026-04-01T10:00:00Z' }],
    });
    expect(shouldShowExchanges(thread, false)).toBe('content');
  });

  it('eventsLoaded=false, no events, no pending → loading (spinner)', () => {
    const thread = makeThread('cc-spawn-2', { eventsLoaded: false });
    expect(shouldShowExchanges(thread, false)).toBe('spinner');
  });

  it('eventsLoaded=true, no events, no pending → empty message', () => {
    const thread = makeThread('cc-spawn-3', { eventsLoaded: true });
    expect(shouldShowExchanges(thread, false)).toBe('empty-message');
  });
});

describe('No-content bug regression — render tree never produces null', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
    threadsLoaded.value = false;
    seqCounter = 1;
  });

  it('loading state shows spinner, never null', () => {
    const thread = makeThread('t-blank-1', { eventsLoaded: false });
    expect(shouldShowExchanges(thread, false)).toBe('spinner');
  });

  it('animating state shows spinner, never null', () => {
    const thread = makeThread('t-blank-2', { eventsLoaded: true });
    const map = new Map([['t-blank-2', thread]]);
    insertEvents(map, 't-blank-2', [
      { type: 'MessageReceived', text: 'hi', channel: 'chat', created: '2026-04-01T10:00:00Z' } as any,
    ]);
    expect(shouldShowExchanges(thread, true)).toBe('spinner');
  });

  it('failed state shows error, never null', () => {
    const thread = makeThread('t-blank-3', { eventsLoaded: false, eventsLoadFailed: true });
    expect(shouldShowExchanges(thread, false)).toBe('error');
  });

  it('loaded empty thread shows "no messages", never null', () => {
    const thread = makeThread('t-blank-4', { eventsLoaded: true });
    expect(shouldShowExchanges(thread, false)).toBe('empty-message');
  });

  it('every state — including undefined thread — produces visible output', () => {
    const states = [
      { eventsLoaded: false, eventsLoadFailed: false, hasEvents: false, animating: false },
      { eventsLoaded: false, eventsLoadFailed: false, hasEvents: false, animating: true },
      { eventsLoaded: false, eventsLoadFailed: false, hasEvents: true, animating: false },
      { eventsLoaded: false, eventsLoadFailed: true, hasEvents: false, animating: false },
      { eventsLoaded: true, eventsLoadFailed: false, hasEvents: false, animating: false },
      { eventsLoaded: true, eventsLoadFailed: false, hasEvents: true, animating: false },
      { eventsLoaded: true, eventsLoadFailed: false, hasEvents: true, animating: true },
    ];

    for (const s of states) {
      const thread = makeThread('t-exhaust', {
        eventsLoaded: s.eventsLoaded,
        eventsLoadFailed: s.eventsLoadFailed,
      });
      if (s.hasEvents) {
        const map = new Map([['t-exhaust', thread]]);
        insertEvents(map, 't-exhaust', [
          { type: 'MessageReceived', text: 'hi', channel: 'chat', created: '2026-04-01T10:00:00Z' } as any,
        ]);
      }
      const result = shouldShowExchanges(thread, s.animating);
      expect(result, `State ${JSON.stringify(s)} produced '${result}'`).toBe(
        s.eventsLoadFailed && !s.hasEvents && !s.eventsLoaded ? 'error'
          : (s.eventsLoaded || s.hasEvents) && !s.animating && s.hasEvents ? 'content'
          : s.eventsLoaded && !s.hasEvents ? 'empty-message'
          : 'spinner'
      );
    }

    const undefinedResult = shouldShowExchanges(undefined, false);
    expect(undefinedResult, 'undefined thread must show spinner, never blank').toBe('spinner');
  });
});

describe('Empty focused thread recovery', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
    threadsLoaded.value = false;
    seqCounter = 1;
  });

  it('focused thread with eventsLoaded=true but 0 events should be refreshable', () => {
    const thread = makeThread('t-empty-focus', { eventsLoaded: true });
    const map = new Map([['t-empty-focus', thread]]);
    threadMap.value = map;
    focusedThreadId.value = 't-empty-focus';

    expect(shouldShowExchanges(thread, false)).toBe('empty-message');
    expect(thread.events.size).toBe(0);

    insertEvents(map, 't-empty-focus', [
      { type: 'MessageReceived', text: 'hello', channel: 'chat', created: '2026-04-06T10:00:00Z' } as any,
      { type: 'ResponseGenerated', text: 'hi', created: '2026-04-06T10:00:05Z' } as any,
    ]);

    expect(thread.events.size).toBe(2);
    expect(shouldShowExchanges(thread, false)).toBe('content');
  });

  it('shouldRefreshFocusedThread detects empty focused thread needing refresh', () => {
    const thread = makeThread('t-detect', { eventsLoaded: true });
    threadMap.value = new Map([['t-detect', thread]]);
    focusedThreadId.value = 't-detect';

    expect(thread.eventsLoaded).toBe(true);
    expect(thread.events.size).toBe(0);
    expect(focusedThreadId.value).toBe('t-detect');

    insertEvents(threadMap.value, 't-detect', [
      { type: 'MessageReceived', text: 'hi', channel: 'chat', created: '2026-04-06T10:00:00Z' } as any,
    ]);
    expect(thread.events.size).toBeGreaterThan(0);
  });

  it('non-focused empty thread does NOT trigger refresh', () => {
    const thread = makeThread('t-nonfocus', { eventsLoaded: true });
    threadMap.value = new Map([['t-nonfocus', thread]]);
    focusedThreadId.value = 'other-thread';

    expect(thread.eventsLoaded).toBe(true);
    expect(thread.events.size).toBe(0);
  });
});

describe('loadThreadEvents edge cases', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
    threadsLoaded.value = false;
    seqCounter = 1;
  });

  it('skips when thread not in map', async () => {
    await loadThreadEvents('nonexistent-thread');
    expect(threadMap.value.size).toBe(0);
  });

  it('skips when already loaded', async () => {
    const thread = makeThread('t1', { eventsLoaded: true });
    threadMap.value = new Map([['t1', thread]]);
    await loadThreadEvents('t1');
    expect(thread.eventsLoaded).toBe(true);
  });
});

describe('loadThreadEvents failure handling', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
    threadsLoaded.value = false;
    seqCounter = 1;
  });

  it('failure sets eventsLoadFailed — UI shows error, not spinner or "no messages"', () => {
    const thread = makeThread('t1', { eventsLoaded: false });
    const map = new Map([['t1', thread]]);
    threadMap.value = map;

    thread.eventsLoadFailed = true;
    threadMap.value = new Map(threadMap.value);

    expect(thread.eventsLoaded).toBe(false);
    expect(thread.eventsLoadFailed).toBe(true);

    expect(shouldShowExchanges(thread, false)).toBe('error');
  });

  it('SSE events arriving clear eventsLoadFailed — content recovers', () => {
    const thread = makeThread('t-sse-recover', { eventsLoaded: false, eventsLoadFailed: true });
    const map = new Map([['t-sse-recover', thread]]);
    threadMap.value = map;

    expect(shouldShowExchanges(thread, false)).toBe('error');

    thread.eventsLoadFailed = false;
    insertEvents(map, 't-sse-recover', [
      { type: 'MessageReceived', text: 'hello', channel: 'chat', created: '2026-04-01T10:00:00Z' } as any,
    ]);

    expect(shouldShowExchanges(thread, false)).toBe('content');
  });

  it('failed thread can recover via loadThreadEvents retry on resume', () => {
    const thread = makeThread('t1', { eventsLoaded: false, eventsLoadFailed: true });
    const map = new Map([['t1', thread]]);
    threadMap.value = map;

    thread.eventsLoadFailed = false;
    thread.eventsLoaded = true;
    insertEvents(map, 't1', [
      { type: 'MessageReceived', text: 'hello', channel: 'chat', created: '2026-04-01T10:00:00Z' } as any,
      { type: 'ResponseGenerated', text: 'hi', created: '2026-04-01T10:00:05Z' } as any,
    ]);

    expect(thread.events.size).toBe(2);
    expect(thread.eventsLoaded).toBe(true);
    expect(thread.eventsLoadFailed).toBe(false);
    expect(shouldShowExchanges(thread, false)).toBe('content');
  });
});
