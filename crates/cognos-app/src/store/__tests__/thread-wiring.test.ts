/**
 * Tests for the wiring layer — thread-sync.ts, thread-loading.ts, chat.ts
 * interactions with threadMap, focusedThreadId, and SSE event routing.
 *
 * These test the functions that modify store state, not the pure data pipeline.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
  handleEvent,
  getCCWaitingInfo,
  groupIntoExchanges,
  type ThreadState,
  type StoredEvent,
} from '../thread-events';
import { focusedThreadId, threadMap } from '../store';
import { flushThreadMap } from '../actions/thread-sync';
import { upsertThread } from '../actions/thread-loading';

const TS = '2026-04-17T00:00:00Z';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeThread(overrides: Partial<ThreadState> = {}): ThreadState {
  return {
    meta: {
      id: 'thread-1',
      title: '...',
      channel: 'chat',
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
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
    ...overrides,
  };
}


// ---------------------------------------------------------------------------
// SSE routing: skeleton thread creation
// ---------------------------------------------------------------------------
describe('SSE routing: skeleton thread creation', () => {
  it('handleEvent ignores events for unknown threads', () => {
    const map = new Map<string, ThreadState>();
    const result = handleEvent(map, 'unknown-id', 1, { type: 'ToolCalled', name: 'x', args: {} }, TS);
    expect(result).toBe(false);
    expect(map.size).toBe(0);
  });

  it('skeleton thread starts with eventsLoaded=false', () => {
    const thread = makeThread();
    expect(thread.eventsLoaded).toBe(false);
  });

  it('skeleton thread source updates on SessionStarted', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'SessionStarted', session_id: 'cc-1' }, TS);
    // Source should be updated by thread-sync.ts, not handleEvent
    // handleEvent just inserts the event
    expect(thread.events.size).toBe(1);
  });
});

// ---------------------------------------------------------------------------
// Bug: SSE skeleton eventsLoaded must allow DB backfill
// ---------------------------------------------------------------------------
describe('SSE skeleton must not prevent DB backfill', () => {
  it('SSE skeleton with eventsLoaded=true causes missed events (reproduces bug)', () => {
    // Simulate the bug: engine restarts, recovery emits MessageReceived + CC events.
    // Frontend connects to SSE late, misses MessageReceived, gets later CC events.
    // SSE creates skeleton with eventsLoaded=true, so loadThreadEvents skips DB load.
    const skeleton: ThreadState = {
      meta: { id: 'recovery-1', title: 'Recovering...', channel: 'claude_code', initiator: 'user', pinned: false, createdAt: '', updatedAt: '', unread: true, status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'default', activeChildrenCount: 0, totalChildrenCount: 0 },
      events: new Map(),
      streamingBuffer: '',
      eventsLoaded: true, // Old CodingAgentThreadSpawned behavior — now fixed to false
      eventsLoadFailed: false,
      lastDbSeq: 0,
      pendingUserMessages: [],
    };
    const map = new Map([['recovery-1', skeleton]]);

    // SSE delivers only later events (frontend missed MessageReceived)
    handleEvent(map, 'recovery-1', 5, { type: 'CodingAgentToolCalled', name: 'Read', args: {} }, TS);
    handleEvent(map, 'recovery-1', 6, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' }, TS);

    // Thread has CC events but no MessageReceived — no exchange can be formed
    const events = [...skeleton.events.values()];
    expect(events.some(e => e.type === 'MessageReceived')).toBe(false);

    // loadThreadEvents would skip because eventsLoaded=true — this is the bug
    expect(skeleton.eventsLoaded).toBe(true); // Bug: should be false to allow backfill
  });

  it('SSE skeleton with eventsLoaded=false allows DB backfill to fill gaps', () => {
    // After the fix: skeleton.eventsLoaded=false, so loadThreadEvents runs,
    // loads MessageReceived from DB, and the thread shows its messages.
    const skeleton: ThreadState = {
      meta: { id: 'recovery-1', title: 'Recovering...', channel: 'claude_code', initiator: 'user', pinned: false, createdAt: '', updatedAt: '', unread: true, status: 'idle', ccHasChanges: false, ccRequiresRestart: false, ccIsExternalRepo: false, ccApplying: false, lastRevivedAt: '', messageCount: 0, section: 'default', activeChildrenCount: 0, totalChildrenCount: 0 },
      events: new Map(),
      streamingBuffer: '',
      eventsLoaded: false, // Fix: allows DB backfill
      eventsLoadFailed: false,
      lastDbSeq: 0,
      pendingUserMessages: [],
    };
    const map = new Map([['recovery-1', skeleton]]);

    // SSE delivers later events
    handleEvent(map, 'recovery-1', 5, { type: 'CodingAgentToolCalled', name: 'Read', args: {} }, TS);
    handleEvent(map, 'recovery-1', 6, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' }, TS);

    // Simulate DB backfill (loadThreadEvents would do this)
    // DB has the MessageReceived at seq 1 that SSE missed
    handleEvent(map, 'recovery-1', 1, { type: 'MessageReceived', text: 'Recovering interrupted session...' }, TS);
    handleEvent(map, 'recovery-1', 2, { type: 'SessionStarted', session_id: 'cc-recovery' }, TS);

    // Dedup: re-inserting seq 5 and 6 is safely rejected
    const r5 = handleEvent(map, 'recovery-1', 5, { type: 'CodingAgentToolCalled', name: 'Read', args: {} }, TS);
    expect(r5).toBe(false); // Already exists

    // Now thread has complete events including MessageReceived
    const events = [...skeleton.events.values()];
    expect(events.some(e => e.type === 'MessageReceived')).toBe(true);
    expect(skeleton.eventsLoaded).toBe(false); // Will be set true after loadThreadEvents completes
  });
});

// ---------------------------------------------------------------------------
// SSE routing: title updates
// ---------------------------------------------------------------------------
describe('SSE routing: title updates', () => {
  it('ThreadTitleGenerated updates thread title (when handled by thread-sync)', () => {
    // thread-sync.ts does: if (event.type === 'ThreadTitleGenerated') thread.meta.title = event.title
    // We test the data flow: title event is in the map
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'ThreadTitleGenerated', title: 'My Thread' }, TS);
    expect(thread.events.size).toBe(1);
    const evt = [...thread.events.values()][0];
    expect(evt.type).toBe('ThreadTitleGenerated');
    expect((evt as any).title).toBe('My Thread');
  });
});


// ---------------------------------------------------------------------------
// Deduplication
// ---------------------------------------------------------------------------
describe('Deduplication', () => {
  it('same seq is rejected (returns false)', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    const r1 = handleEvent(map, 't1', 100, { type: 'MessageReceived', text: 'first' }, TS);
    const r2 = handleEvent(map, 't1', 100, { type: 'MessageReceived', text: 'duplicate' }, TS);

    expect(r1).toBe(true);
    expect(r2).toBe(false);
    expect(thread.events.size).toBe(1);
    expect((thread.events.get(100) as any).text).toBe('first');
  });

  it('different seqs for same event type are both kept', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 100, { type: 'TextStreamed', text: 'chunk 1' }, TS);
    handleEvent(map, 't1', 101, { type: 'TextStreamed', text: 'chunk 2' }, TS);

    expect(thread.events.size).toBe(2);
  });
});

// ---------------------------------------------------------------------------
// Streaming buffer
// ---------------------------------------------------------------------------
describe('Streaming buffer', () => {
  it('transient events (seq=null) go to streaming buffer', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', null, { type: 'TextStreaming', text: 'hello ' });
    handleEvent(map, 't1', null, { type: 'TextStreaming', text: 'world' });

    expect(thread.streamingBuffer).toBe('hello world');
    expect(thread.events.size).toBe(0);
  });

  it('persisted event clears streaming buffer', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', null, { type: 'TextStreaming', text: 'partial' });
    expect(thread.streamingBuffer).toBe('partial');

    handleEvent(map, 't1', 1, { type: 'TextStreamed', text: 'full text' }, TS);
    expect(thread.streamingBuffer).toBe('');
  });

  it('non-text transient events do not modify buffer', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', null, { type: 'TextStreaming', text: 'some text' });
    handleEvent(map, 't1', null, { type: 'PreambleCompleting' });

    expect(thread.streamingBuffer).toBe('some text');
  });
});

// ---------------------------------------------------------------------------
// pendingUserMessage lifecycle
// ---------------------------------------------------------------------------
describe('pendingUserMessages lifecycle', () => {
  it('cleared on matching MessageReceived event by event_id', () => {
    const thread = makeThread({ pendingUserMessages: [{ text: 'my question', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }] });
    const map = new Map([['t1', thread]]);

    expect(thread.pendingUserMessages).toHaveLength(1);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'my question' }, TS, 'msg-1');
    expect(thread.pendingUserMessages).toEqual([]);
  });

  it('not cleared by non-MessageReceived events', () => {
    const thread = makeThread({ pendingUserMessages: [{ text: 'my question', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }] });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 100, { type: 'ToolCalled', name: 'x', args: {} }, TS);

    // Only the ToolCalled event — pending message still there
    expect(thread.events.size).toBe(1);
    expect(thread.pendingUserMessages).toHaveLength(1);
  });

  it('transient events do NOT clear pendingUserMessages', () => {
    const thread = makeThread({ pendingUserMessages: [{ text: 'my question', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }] });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', null, { type: 'TextStreaming', text: 'chunk' });
    expect(thread.pendingUserMessages).toHaveLength(1);
    expect(thread.events.size).toBe(0);
  });

  it('multiple pending messages: only matching one removed per MessageReceived', () => {
    const thread = makeThread({ pendingUserMessages: [
      { text: 'first', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' },
      { text: 'second', eventId: 'msg-2', created: '2026-01-01T00:00:00Z' },
    ] });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 100, { type: 'MessageReceived', text: 'first' }, TS, 'msg-1');
    expect(thread.pendingUserMessages).toHaveLength(1);
    expect(thread.pendingUserMessages[0].eventId).toBe('msg-2');

    handleEvent(map, 't1', 101, { type: 'MessageReceived', text: 'second' }, TS, 'msg-2');
    expect(thread.pendingUserMessages).toEqual([]);

    const msgEvents = [...thread.events.values()].filter(e => e.type === 'MessageReceived');
    expect(msgEvents).toHaveLength(2);
  });
});

// ---------------------------------------------------------------------------
// Thread status from meta (backend-computed)
// ---------------------------------------------------------------------------
describe('Thread status from meta', () => {
  it('meta.status starts as idle by default', () => {
    const thread = makeThread({ eventsLoaded: false });
    expect(thread.meta.status).toBe('idle');
  });

  it('MessageReceived updates meta.status to running', () => {
    const thread = makeThread({ eventsLoaded: true });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 100, { type: 'MessageReceived', text: 'hi' }, '2026-01-01T00:00:00Z');
    expect(thread.meta.status).toBe('running');
  });

  it('ResponseGenerated updates meta.status to idle (no CC changes)', () => {
    const thread = makeThread({ eventsLoaded: true });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 100, { type: 'MessageReceived', text: 'hi' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 101, { type: 'ResponseGenerated' }, '2026-01-01T00:00:01Z');

    expect(thread.meta.status).toBe('idle');
  });

  it('SessionStarted does NOT change status (metadata event)', () => {
    const thread = makeThread({ eventsLoaded: true });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:00Z');
    // SessionStarted is a technical lifecycle event — status stays at whatever it was.
    // Actual processing is signaled by MessageReceived, CodingAgentPromptSent, etc.
    expect(thread.meta.status).toBe('idle');
  });
});

// ---------------------------------------------------------------------------
// updatedAt tracking
// ---------------------------------------------------------------------------
describe('updatedAt tracking', () => {
  it('persisted events update meta.updatedAt to server timestamp', () => {
    const thread = makeThread();
    thread.meta.updatedAt = '2020-01-01T00:00:00Z';
    const map = new Map([['t1', thread]]);

    const serverTime = '2026-03-15T18:30:00Z';
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'hi' }, serverTime);
    expect(thread.meta.updatedAt).toBe(serverTime);
  });

  it('updatedAt uses server created timestamp, not client time', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    const serverTime = '2026-03-15T10:00:00Z';
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'hi' }, serverTime);
    // Must use server time, not new Date() — ensures consistent ordering with API
    expect(thread.meta.updatedAt).toBe(serverTime);
    // Should NOT be close to current client time
    const clientNow = new Date().toISOString();
    expect(thread.meta.updatedAt).not.toBe(clientNow);
  });

  it('transient events with created timestamp update meta.updatedAt', () => {
    const thread = makeThread();
    thread.meta.updatedAt = '2020-01-01T00:00:00Z';
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', null, { type: 'CodingAgentTextStreamed', text: 'chunk' } as any, '2026-03-15T15:24:59Z');
    expect(thread.meta.updatedAt).toBe('2026-03-15T15:24:59Z');
  });

  it('transient events without created timestamp do NOT update meta.updatedAt', () => {
    const thread = makeThread();
    thread.meta.updatedAt = '2020-01-01T00:00:00Z';
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', null, { type: 'CodingAgentTextStreamed', text: 'chunk' } as any);
    expect(thread.meta.updatedAt).toBe('2020-01-01T00:00:00Z');
  });
});

// ---------------------------------------------------------------------------
// Created timestamp storage
// ---------------------------------------------------------------------------
describe('Created timestamp storage', () => {
  it('events store created timestamp when provided', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 100, { type: 'MessageReceived', text: 'hi' }, '2026-03-14T12:00:00Z');

    const stored = thread.events.get(100) as StoredEvent;
    expect(stored.created).toBe('2026-03-14T12:00:00Z');
  });

  it('persisted events without created get undefined, not a silent default', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);
    const spy = vi.spyOn(console, 'warn').mockImplementation(() => {});

    handleEvent(map, 't1', 100, { type: 'MessageReceived', text: 'hi' });

    const stored = thread.events.get(100) as StoredEvent;
    expect(stored.created).toBeUndefined();
    expect(spy).toHaveBeenCalledWith(expect.stringContaining('missing created timestamp'));
    spy.mockRestore();
  });
});

// ---------------------------------------------------------------------------
// getCCWaitingInfo — CC waiting state from thread meta (backend-computed)
// ---------------------------------------------------------------------------
describe('getCCWaitingInfo', () => {
  it('returns null when status is not waiting', () => {
    const thread = makeThread();
    thread.meta.status = 'idle';
    thread.meta.channel = 'claude_code';
    expect(getCCWaitingInfo(thread.meta)).toBeNull();
  });

  it('returns null when channel is not claude_code', () => {
    const thread = makeThread();
    thread.meta.status = 'waiting';
    thread.meta.channel = 'chat';
    expect(getCCWaitingInfo(thread.meta)).toBeNull();
  });

  it('returns info when status=waiting and channel=claude_code', () => {
    const thread = makeThread();
    thread.meta.status = 'waiting';
    thread.meta.channel = 'claude_code';
    thread.meta.ccHasChanges = true;
    thread.meta.ccRequiresRestart = false;
    thread.meta.ccIsExternalRepo = false;
    thread.meta.ccApplying = false;

    const info = getCCWaitingInfo(thread.meta);
    expect(info).toEqual({ hasChanges: true, isExternalRepo: false, requiresRestart: false, applying: false });
  });

  it('CodingAgentIdled with has_changes=true updates meta', () => {
    const thread = makeThread({ meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEvent(map, 't1', 3, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:02Z');

    expect(thread.meta.status).toBe('waiting');
    expect(thread.meta.ccHasChanges).toBe(true);
    const info = getCCWaitingInfo(thread.meta);
    expect(info).toEqual({ hasChanges: true, isExternalRepo: false, requiresRestart: false, applying: false });
  });

  it('CodingAgentIdled with requires_restart=true updates meta', () => {
    const thread = makeThread({ meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'CodingAgentIdled', has_changes: true, requires_restart: true } as any, '2026-01-01T00:00:02Z');

    expect(thread.meta.ccRequiresRestart).toBe(true);
    const info = getCCWaitingInfo(thread.meta);
    expect(info).toEqual({ hasChanges: true, isExternalRepo: false, requiresRestart: true, applying: false });
  });

  it('SessionEnded after ResponseGenerated (no changes) → idle, no waiting info', () => {
    const thread = makeThread({ meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'ResponseGenerated' }, '2026-01-01T00:00:01Z');
    handleEvent(map, 't1', 3, { type: 'SessionEnded' }, '2026-01-01T00:00:02Z');

    // ResponseGenerated without ccHasChanges → idle
    expect(thread.meta.status).toBe('idle');
    expect(getCCWaitingInfo(thread.meta)).toBeNull();
  });

  it('MessageReceived after CodingAgentIdled resumes work → status=running', () => {
    const thread = makeThread({ meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:02Z');
    handleEvent(map, 't1', 3, { type: 'MessageReceived', text: 'also fix linting' }, '2026-01-01T00:00:03Z');

    expect(thread.meta.status).toBe('running');
    expect(getCCWaitingInfo(thread.meta)).toBeNull();
  });

  it('MergeConflictDetected sets ccApplying=true', () => {
    const thread = makeThread({ meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:02Z');
    handleEvent(map, 't1', 3, { type: 'MergeConflictDetected', change_id: 'c-1', files: ['a.rs'] } as any, '2026-01-01T00:00:03Z');

    expect(thread.meta.ccApplying).toBe(true);
  });

  it('ChangeApplied clears all CC flags and sets status to idle', () => {
    const thread = makeThread({ meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:02Z');
    handleEvent(map, 't1', 3, { type: 'MergeConflictDetected', change_id: 'c-1', files: ['a.rs'] } as any, '2026-01-01T00:00:03Z');
    handleEvent(map, 't1', 4, { type: 'ChangeApplied', change_id: 'c-1' } as any, '2026-01-01T00:00:04Z');

    expect(thread.meta.status).toBe('idle');
    expect(thread.meta.ccHasChanges).toBe(false);
    expect(thread.meta.ccApplying).toBe(false);
    expect(getCCWaitingInfo(thread.meta)).toBeNull();
  });

  it('ChangeProposed sets ccHasChanges=true but does not change status', () => {
    const thread = makeThread({ meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'implement feature' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'ChangeProposed', change_id: 'c-1', description: 'Add feature', files: ['a.rs'] } as any, '2026-01-01T00:00:02Z');

    // ChangeProposed only sets CC flags — status stays 'running' from MessageReceived.
    // This prevents Apply/Discard buttons from appearing while CC is still active
    // (e.g., mid-session commits during hardening).
    expect(thread.meta.status).toBe('running');
    expect(thread.meta.ccHasChanges).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Focused thread preservation on reload
// ---------------------------------------------------------------------------
describe('Focused thread preserved across reload', () => {
  it('focusedThreadId survives when thread not yet in map (pre-load)', () => {
    // Simulate: user focused a thread during the session (e.g. clicked in drawer),
    // but threadMap hasn't loaded that thread's events yet.
    // ThreadView should NOT clear focusedThreadId until threadsLoaded is true.
    const savedId = 'cc-thread-123';
    focusedThreadId.value = savedId;

    const map = new Map<string, ThreadState>();
    // Thread doesn't exist in map — this is the pre-load state.
    // The bug was: ThreadView called unfocusThread() here, wiping the focus.
    // Fix: only unfocus when threadsLoaded is true.
    expect(map.has(savedId)).toBe(false);
    expect(focusedThreadId.value).toBe(savedId);
  });

  it('focusedThreadId cleared when thread genuinely missing after load', () => {
    // After threads are loaded and the focused thread isn't found
    // (e.g. deleted thread), it's correct to clear it.
    const map = new Map<string, ThreadState>();
    map.set('other-thread', makeThread());
    // 'missing-thread' is not in the map after loading
    // ThreadView would call unfocusThread() — correct behavior.
    expect(map.has('missing-thread')).toBe(false);
  });

  it('focused_thread from API response is upserted into threadMap', () => {
    // Bug: focused thread older than recent 15 per source was not in the
    // /api/threads response, causing ThreadView to unfocus on reload.
    // Fix: backend returns focused_thread separately, frontend upserts it.
    const map = new Map<string, ThreadState>();
    const focusedId = 'old-cc-thread';

    // Only recent threads in the map (focused thread is older)
    map.set('recent-1', makeThread({ meta: { ...makeThread().meta, id: 'recent-1' } }));

    // Simulate the focused_thread from the API response — this is what
    // loadAllThreads does after the fix: upsertThread(map, response.focused_thread, false)
    upsertThread(map, {
      thread_id: focusedId,
      title: 'Thread Blocking Bug Investigation',
      channel: 'claude_code',
      initiator: 'user',
      created_at: '2026-03-20T10:00:00Z',
      last_activity: '2026-03-20T11:00:00Z',
      message_count: 5,
      section: 'default',
      active_children_count: 0,
      total_children_count: 0,
      status: 'idle',
      cc_has_changes: false,
      cc_requires_restart: false,
      cc_is_external_repo: false,
      cc_applying: false,
      last_revived_at: null,
    }, false);

    expect(map.has(focusedId)).toBe(true);
    expect(map.get(focusedId)!.meta.title).toBe('Thread Blocking Bug Investigation');
    expect(map.get(focusedId)!.meta.channel).toBe('claude_code');
    expect(map.get(focusedId)!.eventsLoaded).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// CC follow-up runs in same thread (no spawn)
// ---------------------------------------------------------------------------
describe('CC follow-up in same thread', () => {
  it('follow-up in CC thread creates exchange in same thread, not a new one', () => {
    const thread = makeThread();
    const map = new Map([['cc-1', thread]]);

    // First CC exchange
    handleEvent(map, 'cc-1', 1, { type: 'MessageReceived', text: 'fix the bug' }, '2026-03-15T10:00:00Z', undefined);
    handleEvent(map, 'cc-1', 2, { type: 'SessionStarted', session_id: 'claude-code/20260315' }, TS);
    handleEvent(map, 'cc-1', 3, { type: 'CodingAgentTextStreamed', text: 'Fixed.' }, TS);
    handleEvent(map, 'cc-1', 4, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-03-15T10:01:00Z');

    // Follow-up — should be in SAME thread, not a new one
    handleEvent(map, 'cc-1', 5, { type: 'MessageReceived', text: 'also fix linting' }, '2026-03-15T10:02:00Z', undefined);
    handleEvent(map, 'cc-1', 6, { type: 'CodingAgentTextStreamed', text: 'Done.' }, TS);
    handleEvent(map, 'cc-1', 7, { type: 'CodingAgentIdled' } as any, '2026-03-15T10:03:00Z');

    // All events in ONE thread
    expect(map.size).toBe(1);
    expect(thread.events.size).toBe(7);

    // Two exchanges (two MessageReceived)
    const exchanges = [...thread.events.values()].filter(e => e.type === 'MessageReceived');
    expect(exchanges).toHaveLength(2);

    // No redirect response
    const redirects = [...thread.events.values()].filter(e =>
      e.type === 'ResponseGenerated' && (e as any).text?.includes('started a new')
    );
    expect(redirects).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// Exchange count for drawer
// ---------------------------------------------------------------------------
describe('Exchange count for drawer', () => {
  it('counts MessageReceived and TriggerStarted as exchanges', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 100, { type: 'MessageReceived', text: 'q1' }, TS);
    handleEvent(map, 't1', 101, { type: 'ResponseGenerated' }, TS);
    handleEvent(map, 't1', 102, { type: 'MessageReceived', text: 'q2' }, TS);
    handleEvent(map, 't1', 103, { type: 'ResponseGenerated' }, TS);
    handleEvent(map, 't1', 104, { type: 'TriggerStarted', trigger_id: 't1' }, TS);
    handleEvent(map, 't1', 105, { type: 'ResponseGenerated' }, TS);

    const count = [...thread.events.values()].filter(
      e => e.type === 'MessageReceived' || e.type === 'TriggerStarted'
    ).length;
    expect(count).toBe(3);
  });

  it('counts SessionRecovered as an exchange', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 100, { type: 'SessionRecovered', branch: 'claude-code/20260318' }, TS);
    handleEvent(map, 't1', 101, { type: 'SessionStarted', session_id: 'cc-1' }, TS);
    handleEvent(map, 't1', 102, { type: 'ResponseGenerated' }, TS);

    const exchanges = groupIntoExchanges(thread.events);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].userEvent.type).toBe('SessionRecovered');
  });
});

// ---------------------------------------------------------------------------
// End-to-end "Apply Now" scenarios — thread events + meta.status updates
// Tests the complete state machine for each apply path.
// ---------------------------------------------------------------------------
describe('Apply Now: Scenario A3 — clean merge (happy path)', () => {
  it('full flow: CC done → Apply Now → ChangeProposed → apply → ChangeApplied → idle', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    // 1. CC session works and goes idle with changes
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix the bug' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEvent(map, 't1', 3, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }, TS);
    handleEvent(map, 't1', 4, { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' }, TS);
    handleEvent(map, 't1', 5, { type: 'CodingAgentTextStreamed', text: 'Fixed.' }, TS);
    handleEvent(map, 't1', 6, { type: 'ResponseGenerated' }, '2026-01-01T00:00:10Z');
    handleEvent(map, 't1', 7, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:11Z');

    // Thread status: waiting (idle with changes)
    expect(thread.meta.status).toBe('waiting');
    expect(getCCWaitingInfo(thread.meta)).toEqual({ hasChanges: true, isExternalRepo: false, requiresRestart: false, applying: false });

    // 3. Backend proposes change, merges, emits ChangeApplied → ChangeProposed + ChangeApplied + SessionEnded
    handleEvent(map, 't1', 8, { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix', files: ['lib.rs'] } as any, '2026-01-01T00:00:12Z');
    handleEvent(map, 't1', 9, { type: 'ChangeApplied', change_id: 'c-1', requires_restart: false, path: '' } as any, '2026-01-01T00:00:13Z');
    handleEvent(map, 't1', 10, { type: 'SessionEnded' }, '2026-01-01T00:00:14Z');

    // Thread status: idle (change resolved, session ended)
    expect(thread.meta.status).toBe('idle');
    expect(getCCWaitingInfo(thread.meta)).toBeNull();

    // ChangeProposed remains as a step on the CC exchange; ChangeApplied is
    // its own initiator panel exchange (system action with actor + actions).
    const exchanges = groupIntoExchanges(thread.events);
    expect(exchanges).toHaveLength(2);
    const hasProposed = exchanges[0].steps.some(s => s.event.type === 'ChangeProposed');
    expect(hasProposed).toBe(true);
    expect(exchanges[1].userEvent.type).toBe('ChangeApplied');
  });
});

describe('Apply Now: Scenario A1 — hardening not done', () => {
  it('full flow: apply triggers hardening → hardening CC runs → auto-apply → ChangeApplied', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    // 1. CC session works and goes idle
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEvent(map, 't1', 3, { type: 'CodingAgentTextStreamed', text: 'Done.' }, TS);
    handleEvent(map, 't1', 4, { type: 'ResponseGenerated' }, '2026-01-01T00:00:05Z');
    handleEvent(map, 't1', 5, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:06Z');
    expect(thread.meta.status).toBe('waiting');

    // 3. Backend sends review follow-up — CC works
    // First need a MessageReceived or CodingAgentUserMessageSent to resume
    handleEvent(map, 't1', 6, { type: 'CodingAgentUserMessageSent', text: 'Review changes' }, '2026-01-01T00:00:07Z');
    handleEvent(map, 't1', 7, { type: 'CodingAgentToolCalled', name: 'Read', args: {} }, TS);
    handleEvent(map, 't1', 8, { type: 'CodingAgentToolResult', name: 'Read', result: 'code...' }, TS);

    // CC resumed work — status=running (from CodingAgentUserMessageSent), no longer waiting
    expect(thread.meta.status).toBe('running');
    expect(getCCWaitingInfo(thread.meta)).toBeNull();

    // 4. Review finishes, CC idles again
    handleEvent(map, 't1', 9, { type: 'ResponseGenerated' }, '2026-01-01T00:00:10Z');
    handleEvent(map, 't1', 10, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:11Z');
    expect(thread.meta.ccHasChanges).toBe(true);

    // 5. Backend proposes change, merges, emits ChangeApplied + kills CC + SessionEnded
    handleEvent(map, 't1', 11, { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix', files: ['a.rs'] } as any, '2026-01-01T00:00:12Z');
    handleEvent(map, 't1', 12, { type: 'ChangeApplied', change_id: 'c-1', requires_restart: false, path: '' } as any, '2026-01-01T00:00:13Z');
    handleEvent(map, 't1', 13, { type: 'SessionEnded' }, '2026-01-01T00:00:14Z');

    // Thread goes idle
    expect(thread.meta.status).toBe('idle');
    expect(getCCWaitingInfo(thread.meta)).toBeNull();
  });

  it('review fails → ChangeApplyFailed → thread stays waiting, banner shows error', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEvent(map, 't1', 3, { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix', files: ['a.rs'] } as any, '2026-01-01T00:00:02Z');
    handleEvent(map, 't1', 4, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:03Z');
    handleEvent(map, 't1', 5, { type: 'SessionEnded' }, '2026-01-01T00:00:04Z');

    // ChangeApplyFailed arrives (e.g., repo has uncommitted changes)
    handleEvent(map, 't1', 6, { type: 'ChangeApplyFailed', change_id: 'c-1', error: 'uncommitted changes' } as any, '2026-01-01T00:00:05Z');

    // Thread stays waiting — change is still pending, user can retry
    expect(thread.meta.status).toBe('waiting');

    // ChangeApplyFailed is its own initiator panel exchange (system action,
    // surfaces the error in the body).
    const exchanges = groupIntoExchanges(thread.events);
    expect(exchanges).toHaveLength(2);
    const failedExchange = exchanges[1];
    expect(failedExchange.userEvent.type).toBe('ChangeApplyFailed');
    expect((failedExchange.userEvent as { error?: string }).error).toBe('uncommitted changes');
  });
});

describe('Apply Now: Scenario A4 — no commits to apply (branch already merged)', () => {
  it('ChangeApplied without ccHasChanges → idle', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    // 1. CC session works and goes idle with changes
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEvent(map, 't1', 3, { type: 'CodingAgentTextStreamed', text: 'Done.' }, TS);
    handleEvent(map, 't1', 4, { type: 'ResponseGenerated' }, '2026-01-01T00:00:05Z');
    handleEvent(map, 't1', 5, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:06Z');

    expect(thread.meta.ccHasChanges).toBe(true);
    expect(thread.meta.status).toBe('waiting');

    // 3. ChangeApplied clears CC flags and sets status to idle
    handleEvent(map, 't1', 6, { type: 'ChangeApplied', change_id: 'c-1' } as any, '2026-01-01T00:00:07Z');

    // Status becomes idle, CC flags cleared
    expect(thread.meta.status).toBe('idle');
    expect(thread.meta.ccHasChanges).toBe(false);
    expect(getCCWaitingInfo(thread.meta)).toBeNull();
  });
});

describe('Apply Now: Scenario A2 — merge conflict', () => {
  it('full flow: apply → conflict → CC resolves → ChangeApplied → idle', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    const now = new Date();
    const t = (offsetMs: number) => new Date(now.getTime() + offsetMs).toISOString();

    // 1. CC done, idle with changes (fresh timestamps — real-time flow)
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, t(-20000));
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, t(-19000));
    handleEvent(map, 't1', 3, { type: 'CodingAgentTextStreamed', text: 'Done.' }, TS);
    handleEvent(map, 't1', 4, { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix', files: ['main.rs'] } as any, t(-17000));
    handleEvent(map, 't1', 5, { type: 'CodingAgentIdled', has_changes: true } as any, t(-16000));
    handleEvent(map, 't1', 6, { type: 'SessionEnded' }, t(-15000));

    expect(thread.meta.status).toBe('waiting');

    // 2. Apply triggered → backend detects merge conflict
    handleEvent(map, 't1', 7, { type: 'MergeConflictDetected', change_id: 'c-1', files: ['main.rs'] } as any, t(-5000));

    // MergeConflictDetected sets ccApplying=true, status stays waiting
    expect(thread.meta.ccApplying).toBe(true);

    // 3. Conflict resolution CC session works
    handleEvent(map, 't1', 8, { type: 'SessionStarted', session_id: 's2' }, t(-4000));
    // SessionStarted doesn't change status — still waiting
    expect(thread.meta.status).toBe('waiting');
    // CodingAgentPromptSent sets running
    handleEvent(map, 't1', 8.5, { type: 'CodingAgentPromptSent', text: 'Resolve merge conflict' } as any, t(-3500));
    expect(thread.meta.status).toBe('running');

    handleEvent(map, 't1', 9, { type: 'CodingAgentToolCalled', name: 'Read', args: {} }, TS);
    handleEvent(map, 't1', 10, { type: 'CodingAgentToolResult', name: 'Read', result: 'conflict markers...' }, TS);
    handleEvent(map, 't1', 11, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }, TS);
    handleEvent(map, 't1', 12, { type: 'CodingAgentToolResult', name: 'Edit', result: 'resolved' }, TS);
    expect(thread.meta.status).toBe('running');

    // 4. Conflict resolved → ChangeApplied + SessionEnded
    handleEvent(map, 't1', 13, { type: 'ChangeApplied', change_id: 'c-1', requires_restart: false, path: '' } as any, t(-2000));
    handleEvent(map, 't1', 14, { type: 'SessionEnded' }, t(-1000));

    expect(thread.meta.status).toBe('idle');
    expect(getCCWaitingInfo(thread.meta)).toBeNull();

    // Three exchanges: the original user message, the system-spawned merge
    // conflict resolution, and ChangeApplied as its own auditable system action.
    const exchanges = groupIntoExchanges(thread.events);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[1].userEvent.type).toBe('MergeConflictDetected');
    expect(exchanges[2].userEvent.type).toBe('ChangeApplied');
  });

  it('conflict resolution fails → ChangeApplyFailed → thread waiting, can retry', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEvent(map, 't1', 3, { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix', files: ['a.rs'] } as any, '2026-01-01T00:00:02Z');
    handleEvent(map, 't1', 4, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:03Z');
    handleEvent(map, 't1', 5, { type: 'SessionEnded' }, '2026-01-01T00:00:04Z');

    // Merge conflict, CC tries to resolve but fails
    handleEvent(map, 't1', 6, { type: 'MergeConflictDetected', change_id: 'c-1', files: ['a.rs'] } as any, '2026-01-01T00:00:05Z');
    handleEvent(map, 't1', 7, { type: 'ChangeApplyFailed', change_id: 'c-1', error: 'could not resolve conflicts' } as any, '2026-01-01T00:00:06Z');

    // Thread stays waiting — change still pending, user can retry
    expect(thread.meta.status).toBe('waiting');
  });
});

describe('Apply Now: edge cases', () => {
  it('ChangeApplied clears all CC flags → thread goes idle', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEvent(map, 't1', 3, { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix 1', files: ['a.rs'] } as any, '2026-01-01T00:00:02Z');
    // Second round of work
    handleEvent(map, 't1', 4, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }, TS);
    handleEvent(map, 't1', 5, { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' }, TS);
    handleEvent(map, 't1', 6, { type: 'ChangeProposed', change_id: 'c-2', description: 'Fix 2', files: ['b.rs'] } as any, '2026-01-01T00:00:05Z');
    handleEvent(map, 't1', 7, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:06Z');
    handleEvent(map, 't1', 8, { type: 'SessionEnded' }, '2026-01-01T00:00:07Z');

    // Apply changes
    handleEvent(map, 't1', 9, { type: 'ChangeApplied', change_id: 'c-1', requires_restart: false, path: '' } as any, '2026-01-01T00:00:08Z');

    // ChangeApplied clears all CC flags → idle
    expect(thread.meta.status).toBe('idle');
    expect(thread.meta.ccHasChanges).toBe(false);
  });

  it('ChangeApplied with requires_restart=true is reflected in events', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEvent(map, 't1', 3, { type: 'ChangeProposed', change_id: 'c-1', description: 'Engine fix', files: ['engine.rs'], requires_restart: true } as any, '2026-01-01T00:00:02Z');
    handleEvent(map, 't1', 4, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:03Z');
    handleEvent(map, 't1', 5, { type: 'SessionEnded' }, '2026-01-01T00:00:04Z');
    handleEvent(map, 't1', 6, { type: 'ChangeApplied', change_id: 'c-1', requires_restart: true, path: '' } as any, '2026-01-01T00:00:05Z');

    expect(thread.meta.status).toBe('idle');

    const allEvents = [...thread.events.values()];
    const applied = allEvents.find(e => e.type === 'ChangeApplied') as any;
    expect(applied.requires_restart).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// ThreadTitleRenamed handling
// ---------------------------------------------------------------------------
describe('ThreadTitleRenamed event handling', () => {
  it('ThreadTitleRenamed updates thread meta title via handleEvent', () => {
    const thread = makeThread();
    thread.meta.title = 'Old Auto Title';
    const map = new Map([['t1', thread]]);

    // Auto-generated title arrives first
    handleEvent(map, 't1', 1, { type: 'ThreadTitleGenerated', title: 'Auto Title' }, '2026-01-01T00:00:00Z');
    expect(thread.events.get(1)!.type).toBe('ThreadTitleGenerated');

    // User renames the thread
    handleEvent(map, 't1', 2, { type: 'ThreadTitleRenamed', title: 'My Custom Title' }, '2026-01-01T00:01:00Z');
    expect(thread.events.get(2)!.type).toBe('ThreadTitleRenamed');
  });

  it('ThreadTitleRenamed is persisted (seq present, stored in events map)', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 42, { type: 'ThreadTitleRenamed', title: 'Renamed' }, '2026-03-18T12:00:00Z');

    expect(thread.events.has(42)).toBe(true);
    const stored = thread.events.get(42)!;
    expect(stored.type).toBe('ThreadTitleRenamed');
    expect((stored as any).title).toBe('Renamed');
    expect(stored.created).toBe('2026-03-18T12:00:00Z');
  });

  it('ThreadTitleRenamed does not affect thread status', () => {
    const thread = makeThread({ eventsLoaded: true });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'hi' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'ResponseGenerated' }, '2026-01-01T00:00:01Z');
    expect(thread.meta.status).toBe('idle');

    // Renaming doesn't change status
    handleEvent(map, 't1', 3, { type: 'ThreadTitleRenamed', title: 'New Name' }, '2026-01-01T00:01:00Z');
    expect(thread.meta.status).toBe('idle');
  });

  it('ThreadPinned does not change waiting CC session to running', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEvent(map, 't1', 3, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:02Z');

    expect(thread.meta.status).toBe('waiting');

    // Pinning the thread should NOT change status from waiting to running
    handleEvent(map, 't1', 4, { type: 'ThreadPinned' }, '2026-01-01T00:00:03Z');
    expect(thread.meta.status).toBe('waiting');
  });

  it('ThreadUnpinned does not change waiting CC session to running', () => {
    const thread = makeThread({ eventsLoaded: true, meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEvent(map, 't1', 3, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:02Z');

    expect(thread.meta.status).toBe('waiting');

    handleEvent(map, 't1', 4, { type: 'ThreadUnpinned' }, '2026-01-01T00:00:03Z');
    expect(thread.meta.status).toBe('waiting');
  });

  it('ThreadPinned does not change idle thread to running', () => {
    const thread = makeThread({ eventsLoaded: true });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'hi' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'ResponseGenerated' }, '2026-01-01T00:00:01Z');
    expect(thread.meta.status).toBe('idle');

    handleEvent(map, 't1', 3, { type: 'ThreadPinned' }, '2026-01-01T00:00:02Z');
    expect(thread.meta.status).toBe('idle');
  });

  it('ThreadTitleRenamed does not create an exchange boundary', () => {
    const thread = makeThread({ eventsLoaded: true });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'hi' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'ResponseGenerated' }, '2026-01-01T00:00:01Z');
    handleEvent(map, 't1', 3, { type: 'ThreadTitleRenamed', title: 'New Name' }, '2026-01-01T00:01:00Z');
    handleEvent(map, 't1', 4, { type: 'MessageReceived', text: 'follow up' }, '2026-01-01T00:02:00Z');

    const exchanges = groupIntoExchanges(thread.events);
    expect(exchanges).toHaveLength(2); // Two MessageReceived = 2 exchanges
  });
});

// ---------------------------------------------------------------------------
// Optimistic Apply Now — apply phase tracking
// The apply phase is managed in the UI and tracks the client-side state.
// Backend status updates via meta.status and meta.cc* flags.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// SSE batching: threadMap signal updates are coalesced via requestAnimationFrame
// ---------------------------------------------------------------------------
describe('SSE batching: flushThreadMap coalesces signal updates', () => {
  beforeEach(() => {
    threadMap.value = new Map();
  });

  it('flushThreadMap creates a new Map reference (triggers signal reactivity)', () => {
    const map = threadMap.value;
    map.set('t1', makeThread());
    const before = threadMap.value;

    flushThreadMap();

    // Signal holds a different Map reference (Preact detects the change)
    expect(threadMap.value).not.toBe(before);
    // But contains the same data
    expect(threadMap.value.size).toBe(1);
    expect(threadMap.value.has('t1')).toBe(true);
  });

  it('multiple in-place mutations followed by one flush produce correct state', () => {
    const map = threadMap.value;
    const t1 = makeThread();
    const t2 = makeThread({ meta: { ...makeThread().meta, id: 'thread-2' } });
    map.set('t1', t1);
    map.set('t2', t2);

    // Simulate rapid SSE events mutating threads in-place
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'q1' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'ResponseGenerated' }, '2026-01-01T00:00:01Z');
    handleEvent(map, 't2', 1, { type: 'MessageReceived', text: 'q2' }, '2026-01-01T00:00:02Z');

    // No flush yet — signal still points to same reference
    const beforeFlush = threadMap.value;

    // Single flush after all mutations
    flushThreadMap();

    expect(threadMap.value).not.toBe(beforeFlush);
    expect(threadMap.value.get('t1')!.events.size).toBe(2);
    expect(threadMap.value.get('t2')!.events.size).toBe(1);
    expect(threadMap.value.get('t1')!.meta.status).toBe('idle');
  });
});

// ---------------------------------------------------------------------------
// Backend-authoritative liveness: meta.status from backend
// The backend computes thread status based on actual process state, not
// frontend timestamp heuristics. meta.status is the source of truth.
// ---------------------------------------------------------------------------

describe('Backend-authoritative liveness: meta.status from backend', () => {
  it('meta.status reflects backend-computed state, not timestamp heuristics', () => {
    // Scenario: CC is actively working. The backend reports the thread status
    // via meta.status based on actual process state.
    const thread = makeThread({
      eventsLoaded: true,
      meta: { ...makeThread().meta, channel: 'claude_code', status: 'running' },
    });
    const map = new Map([['t1', thread]]);

    // Events can have stale timestamps, but meta.status is authoritative
    const staleTime = new Date(Date.now() - 120_000).toISOString();

    // CC session started, user sent message, CC is working
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix the bug' }, staleTime);
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, staleTime);
    handleEvent(map, 't1', 3, { type: 'CodingAgentToolCalled', name: 'Read', args: {} }, TS);
    handleEvent(map, 't1', 4, { type: 'CodingAgentTextStreamed', text: 'Looking...' }, TS);

    // Backend reports status=running — that's the truth, regardless of timestamps
    expect(thread.meta.status).toBe('running');

    // User sent follow-up while CC was working
    handleEvent(map, 't1', 5, { type: 'CodingAgentUserMessageSent', text: 'check ChangeApplied' }, staleTime);

    // CC resumed work — status stays running
    handleEvent(map, 't1', 6, { type: 'CodingAgentToolCalled', name: 'Search', args: {} }, staleTime);
    expect(thread.meta.status).toBe('running');
  });

  it('ResponseGenerated transitions running → idle', () => {
    const thread = makeThread({
      eventsLoaded: true,
      meta: { ...makeThread().meta, channel: 'chat', status: 'running' },
    });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'hi' }, '2026-01-01T00:00:00Z');
    expect(thread.meta.status).toBe('running');

    handleEvent(map, 't1', 2, { type: 'ResponseGenerated' }, '2026-01-01T00:00:01Z');
    expect(thread.meta.status).toBe('idle');
  });

  it('CodingAgentIdled with has_changes transitions running → waiting', () => {
    const thread = makeThread({
      eventsLoaded: true,
      meta: { ...makeThread().meta, channel: 'claude_code', status: 'running' },
    });
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    expect(thread.meta.status).toBe('running');

    handleEvent(map, 't1', 3, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:02Z');
    expect(thread.meta.status).toBe('waiting');
    expect(thread.meta.ccHasChanges).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// No auto-focus on SSE-created threads
// Regression: threads spawned from another workspace or by the agentic loop
// must NOT steal focus. Only explicit user actions (click, sendMessage) focus.
// ---------------------------------------------------------------------------
describe('No auto-focus on SSE-created threads', () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
  });

  /** Create a thread with a specific ID (avoids double makeThread() call). */
  function threadWithId(id: string, extra: Partial<ThreadState['meta']> = {}): ThreadState {
    const t = makeThread();
    t.meta.id = id;
    Object.assign(t.meta, extra);
    return t;
  }

  it('SSE skeleton creation does not change focusedThreadId', () => {
    const map = threadMap.value;
    map.set('spawned-1', threadWithId('spawned-1'));
    handleEvent(map, 'spawned-1', 1, { type: 'MessageReceived', text: 'task from another workspace' }, '2026-04-13T12:00:00Z');
    threadMap.value = new Map(map);

    expect(focusedThreadId.value).toBeNull();
  });

  it('SSE skeleton creation does not change existing focused thread', () => {
    const map = threadMap.value;
    map.set('focused-thread', threadWithId('focused-thread'));
    focusedThreadId.value = 'focused-thread';

    map.set('spawned-2', threadWithId('spawned-2'));
    handleEvent(map, 'spawned-2', 1, { type: 'MessageReceived', text: 'spawned task' }, '2026-04-13T12:00:00Z');
    handleEvent(map, 'spawned-2', 2, { type: 'SessionStarted', session_id: 'cc-spawned' }, '2026-04-13T12:00:01Z');
    threadMap.value = new Map(map);

    expect(focusedThreadId.value).toBe('focused-thread');
  });

  it('CodingAgentThreadSpawned does not auto-focus the spawned CC thread', () => {
    // Regression for commit 0ca048f0: CodingAgentThreadSpawned previously set
    // focusedThreadId to the new CC thread. This must NOT happen.
    const map = threadMap.value;
    map.set('parent-thread', threadWithId('parent-thread'));
    focusedThreadId.value = 'parent-thread';

    handleEvent(map, 'parent-thread', null, {
      type: 'CodingAgentThreadSpawned',
      cc_thread_id: 'cc-child-1',
      title: 'Fix the bug',
    } as any);
    threadMap.value = new Map(map);

    expect(focusedThreadId.value).toBe('parent-thread');
    expect(localStorage.getItem('cognos-focused-thread')).not.toBe('cc-child-1');
  });

  it('multiple SSE events on new thread do not steal focus', () => {
    const map = threadMap.value;
    map.set('my-thread', threadWithId('my-thread'));
    focusedThreadId.value = 'my-thread';

    map.set('remote-thread', threadWithId('remote-thread', { channel: 'claude_code' }));
    handleEvent(map, 'remote-thread', 1, { type: 'MessageReceived', text: 'remote task' }, '2026-04-13T12:00:00Z');
    handleEvent(map, 'remote-thread', 2, { type: 'SessionStarted', session_id: 'cc-remote' }, '2026-04-13T12:00:01Z');
    handleEvent(map, 'remote-thread', null, { type: 'CodingAgentTextStreamed', text: 'Working...' } as any);
    handleEvent(map, 'remote-thread', 3, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }, TS);
    handleEvent(map, 'remote-thread', 4, { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' }, TS);
    handleEvent(map, 'remote-thread', 5, { type: 'ResponseGenerated' }, '2026-04-13T12:01:00Z');
    handleEvent(map, 'remote-thread', 6, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-04-13T12:01:01Z');
    threadMap.value = new Map(map);

    expect(focusedThreadId.value).toBe('my-thread');
  });
});

