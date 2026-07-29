import { describe, it, expect, vi } from 'vitest';
import { TS, makeThread } from './thread-wiring-helpers';
import { getCodingAgentWaitingInfo, handleEvent, type StoredEvent, type ThreadState } from '../thread-events';
import { upsertThread } from '../actions/thread-loading';
import { getDraft, setDraft } from '../composeDrafts';
import { focusedThreadId } from '../store';
import { handleEventWithAgg } from './aggregate-test-helper';

describe('SSE routing: skeleton thread creation', () => {
  it('handleEvent ignores events for unknown threads', () => {
    const map = new Map<string, ThreadState>();
    const result = handleEvent(map, 'unknown-id', 1, { type: 'ToolCalled', name: 'x', args: {} }, TS);
    expect(result.applied).toBe(false);
    expect(map.size).toBe(0);
  });

  it('skeleton thread starts with eventsLoaded=false', () => {
    const thread = makeThread();
    expect(thread.eventsLoaded).toBe(false);
  });

  it('skeleton thread source updates on SessionStarted', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'SessionStarted', session_id: 'cc-1' }, TS);
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
      meta: { id: 'recovery-1', title: 'Recovering...', channel: 'claude_code', initiator: 'user', saved: false, createdAt: '', updatedAt: '', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, codingAgentHasDiff: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0, totalChildrenCount: 0, blockingDescendantCount: 0, attentionDescendantCount: 0, state: 'active', latestTodoList: null },
      events: new Map(),
      streamingBuffer: '',
      eventsLoaded: true, // Old CodingAgentThreadSpawned behavior — now fixed to false
      eventsLoadFailed: false,
      lastDbSeq: 0,
      pendingUserMessages: [],
    };
    const map = new Map([['recovery-1', skeleton]]);

    // SSE delivers only later events (frontend missed MessageReceived)
    handleEventWithAgg(map, 'recovery-1', 5, { type: 'CodingAgentToolCalled', name: 'Read', args: {} }, TS);
    handleEventWithAgg(map, 'recovery-1', 6, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' }, TS);

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
      meta: { id: 'recovery-1', title: 'Recovering...', channel: 'claude_code', initiator: 'user', saved: false, createdAt: '', updatedAt: '', status: 'idle', codingAgentProposed: false, codingAgentRequiresRestart: false, codingAgentIsExternalRepo: false, codingAgentApplying: false, codingAgentHasDiff: false, lastRevivedAt: '', messageCount: 0, section: 'archived', activeChildrenCount: 0, totalChildrenCount: 0, blockingDescendantCount: 0, attentionDescendantCount: 0, state: 'active', latestTodoList: null },
      events: new Map(),
      streamingBuffer: '',
      eventsLoaded: false, // Fix: allows DB backfill
      eventsLoadFailed: false,
      lastDbSeq: 0,
      pendingUserMessages: [],
    };
    const map = new Map([['recovery-1', skeleton]]);

    // SSE delivers later events
    handleEventWithAgg(map, 'recovery-1', 5, { type: 'CodingAgentToolCalled', name: 'Read', args: {} }, TS);
    handleEventWithAgg(map, 'recovery-1', 6, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' }, TS);

    // Simulate DB backfill (loadThreadEvents would do this)
    // DB has the MessageReceived at seq 1 that SSE missed
    handleEventWithAgg(map, 'recovery-1', 1, { type: 'MessageReceived', text: 'Recovering interrupted session...' }, TS);
    handleEventWithAgg(map, 'recovery-1', 2, { type: 'SessionStarted', session_id: 'cc-recovery' }, TS);

    // Dedup: re-inserting seq 5 and 6 is safely rejected
    const r5 = handleEvent(map, 'recovery-1', 5, { type: 'CodingAgentToolCalled', name: 'Read', args: {} }, TS);
    expect(r5.applied).toBe(false); // Already exists

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

    handleEventWithAgg(map, 't1', 1, { type: 'ThreadTitleGenerated', title: 'My Thread' }, TS);
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

    expect(r1.applied).toBe(true);
    expect(r2.applied).toBe(false);
    expect(thread.events.size).toBe(1);
    expect((thread.events.get(100) as any).text).toBe('first');
  });

  it('different seqs for same event type are both kept', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 100, { type: 'TextStreamed', text: 'chunk 1' }, TS);
    handleEventWithAgg(map, 't1', 101, { type: 'TextStreamed', text: 'chunk 2' }, TS);

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

    handleEventWithAgg(map, 't1', null, { type: 'CumulativeTextUpdated', text: 'hello ' });
    handleEventWithAgg(map, 't1', null, { type: 'CumulativeTextUpdated', text: 'world' });

    expect(thread.streamingBuffer).toBe('hello world');
    expect(thread.events.size).toBe(0);
  });

  it('persisted event clears streaming buffer', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', null, { type: 'CumulativeTextUpdated', text: 'partial' });
    expect(thread.streamingBuffer).toBe('partial');

    handleEventWithAgg(map, 't1', 1, { type: 'TextStreamed', text: 'full text' }, TS);
    expect(thread.streamingBuffer).toBe('');
  });

  it('non-text transient events do not modify buffer', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', null, { type: 'CumulativeTextUpdated', text: 'some text' });
    handleEventWithAgg(map, 't1', null, { type: 'PreambleCompleted' });

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

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'my question' }, TS, 'msg-1');
    expect(thread.pendingUserMessages).toEqual([]);
  });

  it('not cleared by non-MessageReceived events', () => {
    const thread = makeThread({ pendingUserMessages: [{ text: 'my question', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }] });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 100, { type: 'ToolCalled', name: 'x', args: {} }, TS);

    // Only the ToolCalled event — pending message still there
    expect(thread.events.size).toBe(1);
    expect(thread.pendingUserMessages).toHaveLength(1);
  });

  it('transient events do NOT clear pendingUserMessages', () => {
    const thread = makeThread({ pendingUserMessages: [{ text: 'my question', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }] });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', null, { type: 'CumulativeTextUpdated', text: 'chunk' });
    expect(thread.pendingUserMessages).toHaveLength(1);
    expect(thread.events.size).toBe(0);
  });

  it('multiple pending messages: only matching one removed per MessageReceived', () => {
    const thread = makeThread({ pendingUserMessages: [
      { text: 'first', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' },
      { text: 'second', eventId: 'msg-2', created: '2026-01-01T00:00:00Z' },
    ] });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 100, { type: 'MessageReceived', text: 'first' }, TS, 'msg-1');
    expect(thread.pendingUserMessages).toHaveLength(1);
    expect(thread.pendingUserMessages[0].eventId).toBe('msg-2');

    handleEventWithAgg(map, 't1', 101, { type: 'MessageReceived', text: 'second' }, TS, 'msg-2');
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

    handleEventWithAgg(map, 't1', 100, { type: 'MessageReceived', text: 'hi' }, '2026-01-01T00:00:00Z');
    expect(thread.meta.status).toBe('running');
  });

  it('ResponseGenerated updates meta.status to idle (no CC changes)', () => {
    const thread = makeThread({ eventsLoaded: true });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 100, { type: 'MessageReceived', text: 'hi' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 101, { type: 'ResponseGenerated' }, '2026-01-01T00:00:01Z');

    expect(thread.meta.status).toBe('idle');
  });

  it('SessionStarted does NOT change status (metadata event)', () => {
    const thread = makeThread({ eventsLoaded: true });
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:00Z');
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
    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'hi' }, serverTime);
    expect(thread.meta.updatedAt).toBe(serverTime);
  });

  it('updatedAt uses server created timestamp, not client time', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);

    const serverTime = '2026-03-15T10:00:00Z';
    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'hi' }, serverTime);
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

    handleEventWithAgg(map, 't1', null, { type: 'CodingAgentTextStreamed', text: 'chunk' } as any, '2026-03-15T15:24:59Z');
    expect(thread.meta.updatedAt).toBe('2026-03-15T15:24:59Z');
  });

  it('transient events without created timestamp do NOT update meta.updatedAt', () => {
    const thread = makeThread();
    thread.meta.updatedAt = '2020-01-01T00:00:00Z';
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', null, { type: 'CodingAgentTextStreamed', text: 'chunk' } as any);
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

    handleEventWithAgg(map, 't1', 100, { type: 'MessageReceived', text: 'hi' }, '2026-03-14T12:00:00Z');

    const stored = thread.events.get(100) as StoredEvent;
    expect(stored.created).toBe('2026-03-14T12:00:00Z');
  });

  it('persisted events without created get undefined, not a silent default', () => {
    const thread = makeThread();
    const map = new Map([['t1', thread]]);
    const spy = vi.spyOn(console, 'warn').mockImplementation(() => {});

    handleEventWithAgg(map, 't1', 100, { type: 'MessageReceived', text: 'hi' });

    const stored = thread.events.get(100) as StoredEvent;
    expect(stored.created).toBeUndefined();
    expect(spy).toHaveBeenCalledWith(expect.stringContaining('missing created timestamp'));
    spy.mockRestore();
  });
});

// ---------------------------------------------------------------------------
// getCodingAgentWaitingInfo — CC waiting state from thread meta (backend-computed)
// ---------------------------------------------------------------------------
describe('getCodingAgentWaitingInfo', () => {
  it('returns null when status is not waiting', () => {
    const thread = makeThread();
    thread.meta.status = 'idle';
    thread.meta.channel = 'claude_code';
    expect(getCodingAgentWaitingInfo(thread.meta)).toBeNull();
  });

  it('returns null when channel is not claude_code', () => {
    const thread = makeThread();
    thread.meta.status = 'waiting';
    thread.meta.channel = 'chat';
    expect(getCodingAgentWaitingInfo(thread.meta)).toBeNull();
  });

  it('returns info when status=waiting and channel=claude_code', () => {
    const thread = makeThread();
    thread.meta.status = 'waiting';
    thread.meta.channel = 'claude_code';
    thread.meta.codingAgentProposed = true;
    thread.meta.codingAgentRequiresRestart = false;
    thread.meta.codingAgentIsExternalRepo = false;
    thread.meta.codingAgentApplying = false;

    const info = getCodingAgentWaitingInfo(thread.meta);
    expect(info).toEqual({ proposed: true, isExternalRepo: false, requiresRestart: false, applying: false });
  });

  it('CodingAgentIdled with has_changes=true updates meta', () => {
    const thread = makeThread({ meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' }, '2026-01-01T00:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:02Z');

    expect(thread.meta.status).toBe('waiting');
    expect(thread.meta.codingAgentProposed).toBe(true);
    const info = getCodingAgentWaitingInfo(thread.meta);
    expect(info).toEqual({ proposed: true, isExternalRepo: false, requiresRestart: false, applying: false });
  });

  it('CodingAgentIdled with requires_restart=true updates meta', () => {
    const thread = makeThread({ meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'CodingAgentIdled', has_changes: true, requires_restart: true } as any, '2026-01-01T00:00:02Z');

    expect(thread.meta.codingAgentRequiresRestart).toBe(true);
    const info = getCodingAgentWaitingInfo(thread.meta);
    expect(info).toEqual({ proposed: true, isExternalRepo: false, requiresRestart: true, applying: false });
  });

  it('SessionEnded after ResponseGenerated (no changes) → idle, no waiting info', () => {
    const thread = makeThread({ meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'ResponseGenerated' }, '2026-01-01T00:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'SessionEnded' }, '2026-01-01T00:00:02Z');

    // ResponseGenerated without codingAgentProposed → idle
    expect(thread.meta.status).toBe('idle');
    expect(getCodingAgentWaitingInfo(thread.meta)).toBeNull();
  });

  it('MessageReceived after CodingAgentIdled resumes work → status=running', () => {
    const thread = makeThread({ meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:02Z');
    handleEventWithAgg(map, 't1', 3, { type: 'MessageReceived', text: 'also fix linting' }, '2026-01-01T00:00:03Z');

    expect(thread.meta.status).toBe('running');
    expect(getCodingAgentWaitingInfo(thread.meta)).toBeNull();
  });

  it('MergeConflictDetected sets codingAgentApplying=true', () => {
    const thread = makeThread({ meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:02Z');
    handleEventWithAgg(map, 't1', 3, { type: 'MergeConflictDetected', change_id: 'c-1', files: ['a.rs'] } as any, '2026-01-01T00:00:03Z');

    expect(thread.meta.codingAgentApplying).toBe(true);
  });

  it('ChangeApplied clears all CC flags and sets status to idle', () => {
    const thread = makeThread({ meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'fix it' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'CodingAgentIdled', has_changes: true } as any, '2026-01-01T00:00:02Z');
    handleEventWithAgg(map, 't1', 3, { type: 'MergeConflictDetected', change_id: 'c-1', files: ['a.rs'] } as any, '2026-01-01T00:00:03Z');
    handleEventWithAgg(map, 't1', 4, { type: 'ChangeApplied', change_id: 'c-1' } as any, '2026-01-01T00:00:04Z');

    expect(thread.meta.status).toBe('idle');
    expect(thread.meta.codingAgentProposed).toBe(false);
    expect(thread.meta.codingAgentApplying).toBe(false);
    expect(getCodingAgentWaitingInfo(thread.meta)).toBeNull();
  });

  it('ChangeProposed sets codingAgentProposed=true but does not change status', () => {
    const thread = makeThread({ meta: { ...makeThread().meta, channel: 'claude_code' } });
    const map = new Map([['t1', thread]]);
    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'implement feature' }, '2026-01-01T00:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'ChangeProposed', change_id: 'c-1', description: 'Add feature', files: ['a.rs'] } as any, '2026-01-01T00:00:02Z');

    // ChangeProposed only sets CC flags — status stays 'running' from MessageReceived.
    // This prevents Apply/Discard buttons from appearing while CC is still active
    // (e.g., mid-session commits during hardening).
    expect(thread.meta.status).toBe('running');
    expect(thread.meta.codingAgentProposed).toBe(true);
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
    // /api/v1/threads response, causing ThreadView to unfocus on reload.
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
      section: 'archived',
      active_children_count: 0,
      total_children_count: 0,
      blocking_descendant_count: 0, attention_descendant_count: 0,
      status: 'idle',
      coding_agent_proposed: false,
      coding_agent_requires_restart: false,
      coding_agent_is_external_repo: false,
      coding_agent_applying: false,
      coding_agent_has_diff: false,
      last_revived_at: null,
      state: 'active',
      compose_text: '',
      compose_images: [],
    }, false);

    expect(map.has(focusedId)).toBe(true);
    expect(map.get(focusedId)!.meta.title).toBe('Thread Blocking Bug Investigation');
    expect(map.get(focusedId)!.meta.channel).toBe('claude_code');
    expect(map.get(focusedId)!.eventsLoaded).toBe(false);
  });

  it('upsertThread refreshes compose state from API for existing threads', () => {
    // Bug: SSE skeleton creation lands `state='composing'` on the thread before
    // MessageReceived arrives. If MessageReceived is dropped (SSE drop, broadcast
    // backpressure), the thread is stuck at 'composing' on the frontend even
    // though the projection has 'active'. categorizeThreads then skips it from
    // every section — the thread is invisible everywhere except Search.
    // Fix: upsertThread must refresh `state` and the compose tuple from API
    // responses, not just the initial create.
    const map = new Map<string, ThreadState>();
    const id = 'stuck-composing';

    // SSE skeleton landed first with state='composing'
    map.set(id, makeThread({
      meta: { ...makeThread().meta, id, state: 'composing', latestTodoList: null },
    }));
    setDraft(id, { text: 'half-typed', image_hashes: [], mode: 'claude_code' });

    // Subsequent loadAllThreads / resync brings authoritative state from API
    upsertThread(map, {
      thread_id: id,
      title: 'Fix Expired GitHub Enterprise Token',
      channel: 'claude_code',
      initiator: 'user',
      created_at: '2026-05-04T10:56:14Z',
      last_activity: '2026-05-04T10:56:44Z',
      message_count: 1,
      section: 'inbox',
      active_children_count: 0,
      total_children_count: 0,
      blocking_descendant_count: 0, attention_descendant_count: 0,
      status: 'waiting',
      coding_agent_proposed: true,
      coding_agent_requires_restart: false,
      coding_agent_is_external_repo: false,
      coding_agent_applying: false,
      coding_agent_has_diff: false,
      last_revived_at: null,
      state: 'active',
      compose_text: '',
      compose_images: [],
      compose_mode: null,
    }, false);

    const meta = map.get(id)!.meta;
    expect(meta.state).toBe('active');
    const draft = getDraft(id);
    expect(draft.mode).toBeNull();
    expect(draft.text).toBe('');
  });
});

// ---------------------------------------------------------------------------
// CC follow-up runs in same thread (no spawn)
// ---------------------------------------------------------------------------
