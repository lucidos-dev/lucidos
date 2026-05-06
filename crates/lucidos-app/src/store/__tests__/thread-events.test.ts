import { describe, it, expect } from 'vitest';
import {
  groupIntoExchanges,
  handleEvent,
  exchangeResponseEvents,
  exchangeSteps,
  exchangeStatus,
  isAbortedByRestart,
  computeExchanges,
  type ThreadEvent,
  type TransientEvent,
  type ThreadState,
  type ThreadMeta,
} from '../thread-events';

const TS = '2026-04-17T00:00:00Z';

function makeThreadState(events: Map<number, ThreadEvent> = new Map()): ThreadState {
  const meta: ThreadMeta = {
    id: 'thread-1',
    title: 'Test Thread',
    channel: 'chat',
    initiator: 'user',
    pinned: false,
    createdAt: '2026-01-01T00:00:00.000Z',
    updatedAt: '2026-01-01T00:00:00.000Z',
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
  };
  return { meta, events, streamingBuffer: '', eventsLoaded: true, eventsLoadFailed: false, lastDbSeq: 0, pendingUserMessages: [] };
}

// ===========================================================================
// Status derivation via handleEvent (backend-computed)
// ===========================================================================
describe('status updates via handleEvent', () => {
  it('updates status to running on MessageReceived', () => {
    const thread = makeThreadState();
    thread.meta.status = 'idle';
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'MessageReceived', text: 'hi' }, TS);
    expect(thread.meta.status).toBe('running');
  });

  it('updates status to idle on ResponseGenerated (no CC changes)', () => {
    const thread = makeThreadState();
    thread.meta.status = 'running';
    thread.meta.ccHasChanges = false;
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'ResponseGenerated' }, TS);
    expect(thread.meta.status).toBe('idle');
  });

  it('updates status to waiting on ResponseGenerated (with CC changes)', () => {
    const thread = makeThreadState();
    thread.meta.status = 'running';
    thread.meta.ccHasChanges = true;
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'ResponseGenerated' }, TS);
    expect(thread.meta.status).toBe('waiting');
  });

  it('updates status to waiting on CodingAgentIdled', () => {
    const thread = makeThreadState();
    thread.meta.status = 'running';
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'CodingAgentIdled', has_changes: true }, TS);
    expect(thread.meta.status).toBe('waiting');
    expect(thread.meta.ccHasChanges).toBe(true);
  });

  it('updates status to idle on ChangeApplied', () => {
    const thread = makeThreadState();
    thread.meta.status = 'waiting';
    thread.meta.ccHasChanges = true;
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'ChangeApplied', change_id: 'c-1' }, TS);
    expect(thread.meta.status).toBe('idle');
    expect(thread.meta.ccHasChanges).toBe(false);
    expect(thread.meta.ccRequiresRestart).toBe(false);
    expect(thread.meta.ccIsExternalRepo).toBe(false);
    expect(thread.meta.ccApplying).toBe(false);
  });

  it('updates status to idle on ChangeDiscarded', () => {
    const thread = makeThreadState();
    thread.meta.status = 'waiting';
    thread.meta.ccHasChanges = true;
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'ChangeDiscarded', change_id: 'c-1' }, TS);
    expect(thread.meta.status).toBe('idle');
    expect(thread.meta.ccHasChanges).toBe(false);
  });

  it('updates status to failed on ResponseFailed', () => {
    const thread = makeThreadState();
    thread.meta.status = 'running';
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'ResponseFailed', error: 'timeout' }, TS);
    expect(thread.meta.status).toBe('failed');
  });

  it('updates status to idle on SessionEnded', () => {
    const thread = makeThreadState();
    thread.meta.status = 'running';
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'SessionEnded' }, TS);
    expect(thread.meta.status).toBe('idle');
  });

  it('sets ccApplying on MergeConflictDetected', () => {
    const thread = makeThreadState();
    thread.meta.ccApplying = false;
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'MergeConflictDetected', change_id: 'c-1' }, TS);
    expect(thread.meta.ccApplying).toBe(true);
  });

  it('clears ccApplying on ChangeApplyFailed', () => {
    const thread = makeThreadState();
    thread.meta.ccApplying = true;
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'ChangeApplyFailed', change_id: 'c-1', error: 'uncommitted changes' }, TS);
    expect(thread.meta.ccApplying).toBe(false);
  });
});

// ===========================================================================
// groupIntoExchanges
// ===========================================================================
describe('groupIntoExchanges', () => {
  it('groups by MessageReceived boundaries', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'first' }],
      [2, { type: 'TextStreamed', text: 'reply1' }],
      [3, { type: 'ResponseGenerated' }],
      [4, { type: 'MessageReceived', text: 'second' }],
      [5, { type: 'TextStreamed', text: 'reply2' }],
      [6, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);

    expect(exchanges[0].userEvent).toEqual({ type: 'MessageReceived', text: 'first' });
    expect(exchanges[0].userSeq).toBe(1);
    expect(exchanges[0].steps).toHaveLength(2);
    expect(exchanges[0].steps[0]).toEqual({ seq: 2, event: { type: 'TextStreamed', text: 'reply1' } });
    expect(exchanges[0].steps[1]).toEqual({ seq: 3, event: { type: 'ResponseGenerated' } });

    expect(exchanges[1].userEvent).toEqual({ type: 'MessageReceived', text: 'second' });
    expect(exchanges[1].userSeq).toBe(4);
    expect(exchanges[1].steps).toHaveLength(2);
  });

  it('handles TriggerStarted as exchange boundary', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'TriggerStarted', trigger_id: 'task-1' }],
      [2, { type: 'TextStreamed', text: 'working...' }],
      [3, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].userEvent).toEqual({ type: 'TriggerStarted', trigger_id: 'task-1' });
    expect(exchanges[0].userSeq).toBe(1);
    expect(exchanges[0].steps).toHaveLength(2);
  });

  it('skips orphaned events before first exchange', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'TextStreamed', text: 'orphan' }],
      [2, { type: 'ResponseGenerated' }],
      [3, { type: 'MessageReceived', text: 'real start' }],
      [4, { type: 'TextStreamed', text: 'reply' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].userSeq).toBe(3);
    expect(exchanges[0].steps).toHaveLength(1);
  });

  it('handles empty map', () => {
    const exchanges = groupIntoExchanges(new Map());
    expect(exchanges).toHaveLength(0);
  });

  it('handles single message with no response', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'hello?' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].userEvent).toEqual({ type: 'MessageReceived', text: 'hello?' });
    expect(exchanges[0].steps).toHaveLength(0);
  });

  // Change lifecycle events are system-initiated mutations on the project
  // (apply / discard / revert / fail). They render as their own initiator
  // panels so the actor is visible at the top-level timeline, not buried
  // inside the previous CC response.
  it.each([
    'ChangeApplied',
    'ChangeDiscarded',
    'ChangeReverted',
    'ChangeApplyFailed',
  ] as const)('treats %s as an exchange-starting event', (type) => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'apply this' }],
      [2, { type: 'CodingAgentIdled', has_changes: true }],
      [3, { type, change_id: 'c1', error: 'x' } as ThreadEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe(type);
    expect(exchanges[1].userSeq).toBe(3);
    expect(exchanges[1].steps).toHaveLength(0);
  });
});

// ===========================================================================
// handleEvent
// ===========================================================================
describe('handleEvent', () => {
  it('inserts persisted events', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);
    const event: ThreadEvent = { type: 'MessageReceived', text: 'hi' };

    const result = handleEvent(threadMap, 'thread-1', 1, event, TS);

    expect(result).toBe(true);
    expect(thread.events.get(1)).toEqual(expect.objectContaining(event));
  });

  it('deduplicates by sequence number', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);
    const event1: ThreadEvent = { type: 'MessageReceived', text: 'hi' };
    const event2: ThreadEvent = { type: 'MessageReceived', text: 'different' };

    handleEvent(threadMap, 'thread-1', 1, event1, TS);
    const result = handleEvent(threadMap, 'thread-1', 1, event2, TS);

    expect(result).toBe(false);
    expect(thread.events.get(1)).toEqual(expect.objectContaining(event1)); // original kept
  });

  it('appends transient text to streaming buffer', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);
    const transient: TransientEvent = { type: 'TextStreaming', text: 'hel' };

    handleEvent(threadMap, 'thread-1', null, transient);
    expect(thread.streamingBuffer).toBe('hel');

    handleEvent(threadMap, 'thread-1', null, { type: 'TextStreaming', text: 'lo' });
    expect(thread.streamingBuffer).toBe('hello');
  });

  it('clears streaming buffer on persisted event', () => {
    const thread = makeThreadState();
    thread.streamingBuffer = 'partial text';
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'TextStreamed', text: 'full text' }, TS);
    expect(thread.streamingBuffer).toBe('');
  });

  it('ignores unknown threads', () => {
    const threadMap = new Map<string, ThreadState>();
    const result = handleEvent(threadMap, 'nonexistent', 1, { type: 'MessageReceived', text: 'hi' }, TS);
    expect(result).toBe(false);
  });

  it('updates updatedAt on persisted events with server timestamp', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);

    const serverTime = '2026-03-15T12:00:00Z';
    handleEvent(threadMap, 'thread-1', 1, { type: 'MessageReceived', text: 'hi' }, serverTime);
    expect(thread.meta.updatedAt).toBe(serverTime);
  });

  it('updates updatedAt on ChangeApplied (persisted non-metadata event)', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);

    const t1 = '2026-03-15T12:00:00Z';
    handleEvent(threadMap, 'thread-1', 1, { type: 'MessageReceived', text: 'hi' }, t1);
    expect(thread.meta.updatedAt).toBe(t1);

    const t2 = '2026-03-15T12:05:00Z';
    handleEvent(threadMap, 'thread-1', 2, { type: 'ChangeApplied', change_id: 'c1' }, t2);
    expect(thread.meta.updatedAt).toBe(t2);
  });

  it('updates updatedAt on CodingAgentTextStreamed (persisted CC step event)', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);

    const t1 = '2026-03-15T12:00:00Z';
    handleEvent(threadMap, 'thread-1', 1, { type: 'SessionStarted', session_id: 's1' }, t1);

    const t2 = '2026-03-15T12:05:00Z';
    handleEvent(threadMap, 'thread-1', 2, { type: 'CodingAgentTextStreamed', text: 'working...' }, t2);
    expect(thread.meta.updatedAt).toBe(t2);
  });

  it('updates updatedAt on CodingAgentToolCalled (persisted CC step event)', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);

    const t1 = '2026-03-15T12:00:00Z';
    handleEvent(threadMap, 'thread-1', 1, { type: 'SessionStarted', session_id: 's1' }, t1);

    const t2 = '2026-03-15T12:05:00Z';
    handleEvent(threadMap, 'thread-1', 2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }, t2);
    expect(thread.meta.updatedAt).toBe(t2);
  });

  it('does NOT update updatedAt on CodingAgentPromptSent (backend only updates status)', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const threadMap = new Map([['thread-1', thread]]);

    const t1 = '2026-03-15T12:00:00Z';
    handleEvent(threadMap, 'thread-1', 1, { type: 'MessageReceived', text: 'fix the bug' }, t1);

    const t2 = '2026-03-30T18:00:00Z';
    handleEvent(threadMap, 'thread-1', 2, { type: 'CodingAgentPromptSent', text: 'Run /harden now.' }, t2);
    expect(thread.meta.updatedAt).toBe(t1); // Should NOT update — backend doesn't update last_activity
  });

  it('does NOT update updatedAt on SessionEnded (lifecycle, not activity)', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const threadMap = new Map([['thread-1', thread]]);

    const t1 = '2026-03-15T12:00:00Z';
    handleEvent(threadMap, 'thread-1', 1, { type: 'CodingAgentIdled' }, t1);
    expect(thread.meta.updatedAt).toBe(t1);

    const t2 = '2026-03-15T12:01:43Z';
    handleEvent(threadMap, 'thread-1', 2, { type: 'SessionEnded' }, t2);
    expect(thread.meta.updatedAt).toBe(t1); // SessionEnded should NOT update
  });

  it('does NOT update updatedAt on ChangeProposed (status-only, not activity)', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const threadMap = new Map([['thread-1', thread]]);

    const t1 = '2026-03-15T12:00:00Z';
    handleEvent(threadMap, 'thread-1', 1, { type: 'CodingAgentIdled' }, t1);

    const t2 = '2026-03-15T12:01:00Z';
    handleEvent(threadMap, 'thread-1', 2, { type: 'ChangeProposed', change_id: 'c1' }, t2);
    expect(thread.meta.updatedAt).toBe(t1); // ChangeProposed should NOT update
  });

  it('does NOT update updatedAt on ResponseCanceled (status-only)', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);

    const t1 = '2026-03-15T12:00:00Z';
    handleEvent(threadMap, 'thread-1', 1, { type: 'MessageReceived', text: 'hi' }, t1);

    const t2 = '2026-03-15T12:01:00Z';
    handleEvent(threadMap, 'thread-1', 2, { type: 'ResponseCanceled' }, t2);
    expect(thread.meta.updatedAt).toBe(t1); // ResponseCanceled should NOT update
  });

  it('does NOT update updatedAt on metadata events (ThreadTitleGenerated)', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);

    const t1 = '2026-03-15T12:00:00Z';
    handleEvent(threadMap, 'thread-1', 1, { type: 'MessageReceived', text: 'hi' }, t1);

    const t2 = '2026-03-15T12:10:00Z';
    handleEvent(threadMap, 'thread-1', 2, { type: 'ThreadTitleGenerated', title: 'Test' }, t2);
    expect(thread.meta.updatedAt).toBe(t1); // Should NOT update to t2
  });

  it('clears matching pendingUserMessage on MessageReceived with matching event_id', () => {
    const thread = makeThreadState();
    thread.pendingUserMessages = [{ text: 'optimistic message', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }];
    const threadMap = new Map([['thread-1', thread]]);

    expect(thread.pendingUserMessages).toHaveLength(1);
    handleEvent(threadMap, 'thread-1', 1, { type: 'MessageReceived', text: 'optimistic message' }, TS, 'msg-1');
    expect(thread.pendingUserMessages).toEqual([]);
  });

  it('does not clear pendingUserMessages on transient events', () => {
    const thread = makeThreadState();
    thread.pendingUserMessages = [{ text: 'optimistic message', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }];
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', null, { type: 'TextStreaming', text: 'chunk' });
    expect(thread.pendingUserMessages).toHaveLength(1);
  });

  it('does not clear pendingUserMessages on non-MessageReceived persisted events', () => {
    const thread = makeThreadState();
    thread.pendingUserMessages = [{ text: 'my question', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }];
    const threadMap = new Map([['thread-1', thread]]);

    // A ToolCalled event arrives — should NOT clear pending messages
    handleEvent(threadMap, 'thread-1', 100, { type: 'ToolCalled', name: 'search', args: {} }, TS);

    // pendingUserMessages should still be there
    expect(thread.pendingUserMessages).toHaveLength(1);

    // Only the ToolCalled event — no synthetic MessageReceived
    expect(thread.events.size).toBe(1);
    expect(thread.events.get(100)!.type).toBe('ToolCalled');
  });

  it('only removes the matching pending message by event_id, keeps others', () => {
    const thread = makeThreadState();
    thread.pendingUserMessages = [
      { text: 'first', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' },
      { text: 'second', eventId: 'msg-2', created: '2026-01-01T00:00:00Z' },
      { text: 'third', eventId: 'msg-3', created: '2026-01-01T00:00:00Z' },
    ];
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'MessageReceived', text: 'first' }, TS, 'msg-1');
    expect(thread.pendingUserMessages).toHaveLength(2);
    expect(thread.pendingUserMessages[0].eventId).toBe('msg-2');

    handleEvent(threadMap, 'thread-1', 2, { type: 'MessageReceived', text: 'second' }, TS, 'msg-2');
    expect(thread.pendingUserMessages).toHaveLength(1);
    expect(thread.pendingUserMessages[0].eventId).toBe('msg-3');
  });

  it('clears matching pendingUserMessage on UserPromptInjected with matching event_id', () => {
    const thread = makeThreadState();
    thread.pendingUserMessages = [{ text: 'fix the bug', eventId: 'inject-1', created: '2026-01-01T00:00:00Z' }];
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 10, { type: 'UserPromptInjected', text: 'fix the bug' }, TS, 'inject-1');
    expect(thread.pendingUserMessages).toEqual([]);
  });
});

// ===========================================================================
// UserPromptInjected — status updates
// ===========================================================================
describe('UserPromptInjected status updates', () => {
  it('sets status to running after UserPromptInjected', () => {
    const thread = makeThreadState();
    thread.meta.status = 'idle';
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'UserPromptInjected', text: 'actually do this instead' }, TS);
    expect(thread.meta.status).toBe('running');
  });

  it('sets status to idle when ResponseGenerated follows UserPromptInjected', () => {
    const thread = makeThreadState();
    thread.meta.status = 'running';
    thread.meta.ccHasChanges = false;
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'ResponseGenerated' }, TS);
    expect(thread.meta.status).toBe('idle');
  });
});

// ===========================================================================
// UserPromptInjected — groupIntoExchanges
// ===========================================================================
describe('UserPromptInjected in groupIntoExchanges', () => {
  it('UserPromptInjected starts a new exchange', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'do X' }],
      [2, { type: 'ToolCalled', name: 'web_search', args: {} }],
      [3, { type: 'ToolResult', name: 'web_search', result: 'ok' }],
      [4, { type: 'UserPromptInjected', text: 'actually do Y' }],
      [5, { type: 'TextStreamed', text: 'doing Y now' }],
      [6, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].userEvent).toEqual({ type: 'MessageReceived', text: 'do X' });
    expect(exchanges[0].steps).toHaveLength(2); // ToolCalled + ToolResult
    expect(exchanges[1].userEvent).toEqual({ type: 'UserPromptInjected', text: 'actually do Y' });
    expect(exchanges[1].steps).toHaveLength(2); // TextStreamed + ResponseGenerated
  });

  it('multiple injections create multiple exchanges', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'start' }],
      [2, { type: 'ToolCalled', name: 'search', args: {} }],
      [3, { type: 'UserPromptInjected', text: 'correction 1' }],
      [4, { type: 'ToolCalled', name: 'read', args: {} }],
      [5, { type: 'UserPromptInjected', text: 'correction 2' }],
      [6, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[1].userEvent.type).toBe('UserPromptInjected');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('correction 1');
    expect(exchanges[2].userEvent.type).toBe('UserPromptInjected');
    expect((exchanges[2].userEvent as { text: string }).text).toBe('correction 2');
  });
});

// ===========================================================================
// ThreadMarkedRead event handling
// ===========================================================================
describe('handleEvent — ThreadMarkedRead', () => {
  it('sets section to default when ThreadMarkedRead event is received', () => {
    const state = makeThreadState();
    state.meta.section = 'unread';
    const map = new Map<string, ThreadState>([['thread-1', state]]);

    handleEvent(map, 'thread-1', 100, { type: 'ThreadMarkedRead' } as ThreadEvent, '2026-03-25T10:00:00Z');

    expect(map.get('thread-1')!.meta.section).toBe('default');
  });

  it('ThreadDismissed sets section to default from unread', () => {
    const state = makeThreadState();
    state.meta.section = 'unread';
    const map = new Map<string, ThreadState>([['thread-1', state]]);

    handleEvent(map, 'thread-1', 100, { type: 'ThreadDismissed' } as ThreadEvent, '2026-03-25T10:00:00Z');

    expect(map.get('thread-1')!.meta.section).toBe('default');
  });
});

// ===========================================================================
// Step spinner completion on finished exchanges
// ===========================================================================
describe('step completion — no eternal spinners', () => {
  it('parallel CC tool calls all resolve when results arrive', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    // Simulate: user message, session start, then parallel tool calls with results
    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'fix it' } as ThreadEvent, '2026-04-04T10:00:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-04T10:00:01Z');
    handleEvent(map, 'thread-1', 3, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'a.rs' } } as ThreadEvent, '2026-04-04T10:00:02Z');
    handleEvent(map, 'thread-1', 4, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'b.rs' } } as ThreadEvent, '2026-04-04T10:00:02Z');
    // Only ONE result arrives (CC parallel — result for first call lost)
    handleEvent(map, 'thread-1', 5, { type: 'CodingAgentToolResult', name: '', result: 'ok' } as ThreadEvent, '2026-04-04T10:00:03Z');
    // CC finishes
    handleEvent(map, 'thread-1', 6, { type: 'CodingAgentTextStreamed', text: 'Done!' } as ThreadEvent, '2026-04-04T10:00:04Z');
    handleEvent(map, 'thread-1', 7, { type: 'CodingAgentIdled' } as ThreadEvent, '2026-04-04T10:00:05Z');

    const exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    expect(exchanges).toHaveLength(1);

    const events = exchangeResponseEvents(exchanges[0]);
    const steps = events.filter(e => e.type === 'step');

    // Both steps must be completed (no spinner) — the exchange is done
    for (const step of steps) {
      expect((step as { success: boolean | null }).success).not.toBeNull();
    }
  });

  it('missing ToolResult on completed exchange shows checkmark, not spinner', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'fix it' } as ThreadEvent, '2026-04-04T10:00:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-04T10:00:01Z');
    handleEvent(map, 'thread-1', 3, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, '2026-04-04T10:00:02Z');
    // NO ToolResult — session was killed
    handleEvent(map, 'thread-1', 4, { type: 'CodingAgentIdled' } as ThreadEvent, '2026-04-04T10:00:03Z');

    const exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    const events = exchangeResponseEvents(exchanges[0]);
    const steps = events.filter(e => e.type === 'step');

    expect(steps).toHaveLength(1);
    // Step must NOT show spinner on a completed exchange
    expect((steps[0] as { success: boolean | null }).success).not.toBeNull();
  });

  it('does NOT force-resolve spinners when CC resumed after idle', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'fix it' } as ThreadEvent, '2026-04-04T10:00:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-04T10:00:01Z');
    handleEvent(map, 'thread-1', 3, { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, '2026-04-04T10:00:02Z');
    handleEvent(map, 'thread-1', 4, { type: 'CodingAgentToolResult', name: '', result: 'ok' } as ThreadEvent, '2026-04-04T10:00:03Z');
    // CC idles then resumes with a new tool call (still in progress)
    handleEvent(map, 'thread-1', 5, { type: 'CodingAgentIdled' } as ThreadEvent, '2026-04-04T10:00:04Z');
    handleEvent(map, 'thread-1', 6, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, '2026-04-04T10:00:05Z');
    // No result yet — tool is actively running

    const exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    const events = exchangeResponseEvents(exchanges[0]);
    const steps = events.filter(e => e.type === 'step');

    // The last step should still show spinner — CC is actively working
    const lastStep = steps[steps.length - 1] as { success: boolean | null };
    expect(lastStep.success).toBeNull();
  });

  it('three parallel subagents resolve individually as results arrive (live streaming)', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'fix it' } as ThreadEvent, '2026-04-04T10:00:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-04T10:00:01Z');
    // Three parallel Agent launches
    handleEvent(map, 'thread-1', 3, { type: 'CodingAgentToolCalled', name: 'Agent', args: { prompt: 'task 1' } } as ThreadEvent, '2026-04-04T10:00:02Z');
    handleEvent(map, 'thread-1', 4, { type: 'CodingAgentToolCalled', name: 'Agent', args: { prompt: 'task 2' } } as ThreadEvent, '2026-04-04T10:00:02Z');
    handleEvent(map, 'thread-1', 5, { type: 'CodingAgentToolCalled', name: 'Agent', args: { prompt: 'task 3' } } as ThreadEvent, '2026-04-04T10:00:02Z');

    // First result arrives — should resolve exactly one step
    handleEvent(map, 'thread-1', 6, { type: 'CodingAgentToolResult', name: '', result: 'done 1' } as ThreadEvent, '2026-04-04T10:00:05Z');

    let exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    let events = exchangeResponseEvents(exchanges[0]);
    let steps = events.filter(e => e.type === 'step') as { success: boolean | null }[];
    let resolved = steps.filter(s => s.success === true).length;
    let pending = steps.filter(s => s.success === null).length;
    expect(resolved).toBe(1);
    expect(pending).toBe(2);

    // Second result
    handleEvent(map, 'thread-1', 7, { type: 'CodingAgentToolResult', name: '', result: 'done 2' } as ThreadEvent, '2026-04-04T10:00:06Z');
    exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    events = exchangeResponseEvents(exchanges[0]);
    steps = events.filter(e => e.type === 'step') as { success: boolean | null }[];
    resolved = steps.filter(s => s.success === true).length;
    pending = steps.filter(s => s.success === null).length;
    expect(resolved).toBe(2);
    expect(pending).toBe(1);

    // Third result
    handleEvent(map, 'thread-1', 8, { type: 'CodingAgentToolResult', name: '', result: 'done 3' } as ThreadEvent, '2026-04-04T10:00:07Z');
    exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    events = exchangeResponseEvents(exchanges[0]);
    steps = events.filter(e => e.type === 'step') as { success: boolean | null }[];
    resolved = steps.filter(s => s.success === true).length;
    expect(resolved).toBe(3);
  });

  it('parallel CC tool results resolve individual pending steps (not always the last)', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix it', created: '2026-04-04T10:00:00Z' } as ThreadEvent],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-04-04T10:00:01Z' } as ThreadEvent],
      [3, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'a.rs' }, created: '2026-04-04T10:00:02Z' } as ThreadEvent],
      [4, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'b.rs' }, created: '2026-04-04T10:00:02Z' } as ThreadEvent],
      [5, { type: 'CodingAgentToolCalled', name: 'Grep', args: { pattern: 'foo' }, created: '2026-04-04T10:00:02Z' } as ThreadEvent],
      [6, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: '2026-04-04T10:00:03Z' } as ThreadEvent],
      [7, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: '2026-04-04T10:00:03Z' } as ThreadEvent],
      [8, { type: 'CodingAgentToolResult', name: 'Grep', result: 'ok', created: '2026-04-04T10:00:03Z' } as ThreadEvent],
      [9, { type: 'CodingAgentTextStreamed', text: 'analyzing...', created: '2026-04-04T10:00:04Z' } as ThreadEvent],
    ]);

    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);

    const respEvents = exchangeResponseEvents(exchanges[0]);
    const respSteps = respEvents.filter(e => e.type === 'step');
    expect(respSteps).toHaveLength(3);
    for (const step of respSteps) {
      expect((step as { success: boolean | null }).success).toBe(true);
    }

    const steps = exchangeSteps(exchanges[0]);
    expect(steps).toHaveLength(3);
    for (const step of steps) {
      expect(step.success).toBe(true);
    }
  });

  it('parallel engine tool results resolve individual pending steps', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'search', created: '2026-04-04T10:00:00Z' } as ThreadEvent],
      [2, { type: 'ToolCalled', name: 'web_search', args: { query: 'a' }, created: '2026-04-04T10:00:01Z' } as ThreadEvent],
      [3, { type: 'ToolCalled', name: 'web_search', args: { query: 'b' }, created: '2026-04-04T10:00:01Z' } as ThreadEvent],
      [4, { type: 'ToolResult', name: 'web_search', result: 'res-a', created: '2026-04-04T10:00:02Z' } as ThreadEvent],
      [5, { type: 'ToolResult', name: 'web_search', result: 'res-b', created: '2026-04-04T10:00:02Z' } as ThreadEvent],
      [6, { type: 'TextStreamed', text: 'Here are the results', created: '2026-04-04T10:00:03Z' } as ThreadEvent],
    ]);

    const exchanges = groupIntoExchanges(events);
    const respEvents = exchangeResponseEvents(exchanges[0]);
    const respSteps = respEvents.filter(e => e.type === 'step');
    expect(respSteps).toHaveLength(2);
    for (const step of respSteps) {
      expect((step as { success: boolean | null }).success).toBe(true);
    }

    const steps = exchangeSteps(exchanges[0]);
    expect(steps).toHaveLength(2);
    for (const step of steps) {
      expect(step.success).toBe(true);
    }
  });

  it('exchangeSteps also resolves pending steps on completed exchange', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'fix it' } as ThreadEvent, '2026-04-04T10:00:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, '2026-04-04T10:00:01Z');
    // No ToolResult, but exchange completed
    handleEvent(map, 'thread-1', 3, { type: 'ResponseGenerated' } as ThreadEvent, '2026-04-04T10:00:02Z');

    const exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    const steps = exchangeSteps(exchanges[0]);

    expect(steps).toHaveLength(1);
    expect(steps[0].success).not.toBeNull();
  });
});

// ===========================================================================
// Tool description from event (DRY: backend provides, frontend falls back)
// ===========================================================================
describe('tool description from event', () => {
  it('exchangeSteps uses event description for ToolCalled when present', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'hi' } as ThreadEvent, '2026-04-09T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'ToolCalled', name: 'refresh_app', args: { app_id: 'habit-tracker' }, description: 'Refreshing habit-tracker...' } as ThreadEvent, '2026-04-09T10:00:01Z');
    handleEvent(map, 't', 3, { type: 'ToolResult', name: 'refresh_app', result: 'ok' } as ThreadEvent, '2026-04-09T10:00:02Z');
    handleEvent(map, 't', 4, { type: 'ResponseGenerated' } as ThreadEvent, '2026-04-09T10:00:03Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const steps = exchangeSteps(exchanges[0]);
    expect(steps[0].description).toBe('Refreshing habit-tracker...');
  });

  it('exchangeSteps falls back to local description when event has no description', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'hi' } as ThreadEvent, '2026-04-09T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'ToolCalled', name: 'read_file', args: { path: '/src/main.rs' } } as ThreadEvent, '2026-04-09T10:00:01Z');
    handleEvent(map, 't', 3, { type: 'ToolResult', name: 'read_file', result: 'ok' } as ThreadEvent, '2026-04-09T10:00:02Z');
    handleEvent(map, 't', 4, { type: 'ResponseGenerated' } as ThreadEvent, '2026-04-09T10:00:03Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const steps = exchangeSteps(exchanges[0]);
    expect(steps[0].description).toBe('Read main.rs');
  });

  it('exchangeResponseEvents uses event description for CodingAgentToolCalled when present', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'do it' } as ThreadEvent, '2026-04-09T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: '/src/lib.rs' }, description: 'Read lib.rs' } as ThreadEvent, '2026-04-09T10:00:01Z');
    handleEvent(map, 't', 3, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' } as ThreadEvent, '2026-04-09T10:00:02Z');
    handleEvent(map, 't', 4, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, '2026-04-09T10:00:03Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const respEvents = exchangeResponseEvents(exchanges[0]);
    const stepEvent = respEvents.find(e => e.type === 'step' && e.description === 'Read lib.rs');
    expect(stepEvent).toBeDefined();
  });

  it('exchangeResponseEvents falls back for CodingAgentToolCalled without description', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'do it' } as ThreadEvent, '2026-04-09T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'CodingAgentToolCalled', name: 'Grep', args: { pattern: 'TODO' } } as ThreadEvent, '2026-04-09T10:00:01Z');
    handleEvent(map, 't', 3, { type: 'CodingAgentToolResult', name: 'Grep', result: 'ok' } as ThreadEvent, '2026-04-09T10:00:02Z');
    handleEvent(map, 't', 4, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, '2026-04-09T10:00:03Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const respEvents = exchangeResponseEvents(exchanges[0]);
    const stepEvent = respEvents.find(e => e.type === 'step' && e.description === "Search 'TODO'");
    expect(stepEvent).toBeDefined();
  });

  it('resolves pending steps in non-last exchange when user message splits ToolCalled from its result', () => {
    // Bug: user cancels mid-tool-call → MessageReceived starts a new exchange,
    // orphaning the ToolCalled in the previous exchange without a ToolResult or
    // completion event. The spinner persists forever.
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);

    // Exchange 1: user asks, engine calls a tool
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'find the file' } as ThreadEvent, '2026-04-13T19:30:00Z');
    handleEvent(map, 't', 2, { type: 'Thinking' } as ThreadEvent, '2026-04-13T19:30:01Z');
    handleEvent(map, 't', 3, { type: 'ToolCalled', name: 'bash', args: { command: 'find . -name "Foo.tsx"' } } as ThreadEvent, '2026-04-13T19:30:02Z');
    // User sends a new message BEFORE the tool result arrives — starts Exchange 2
    handleEvent(map, 't', 4, { type: 'MessageReceived', text: 'stop, I found it' } as ThreadEvent, '2026-04-13T19:30:10Z');
    // The tool result and cancellation arrive AFTER the new message
    handleEvent(map, 't', 5, { type: 'ToolResult', name: 'bash', result: 'ok' } as ThreadEvent, '2026-04-13T19:30:15Z');
    handleEvent(map, 't', 6, { type: 'ResponseCanceled' } as ThreadEvent, '2026-04-13T19:30:15Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    expect(exchanges).toHaveLength(2);

    // Verify the step IS pending when treated as the last exchange (the bug scenario)
    const stepsAsLast = exchangeSteps(exchanges[0], true);
    expect(stepsAsLast.filter(s => s.success === null)).toHaveLength(1);

    // Exchange 1 has the ToolCalled but NOT its ToolResult (that ended up in exchange 2).
    // Since it's not the last exchange, pending steps must be resolved.
    const ex1Steps = exchangeSteps(exchanges[0], false);
    expect(ex1Steps.filter(s => s.success === null)).toHaveLength(0);

    const ex1Events = exchangeResponseEvents(exchanges[0], 0, false);
    const pendingEvents = ex1Events.filter(e => e.type === 'step' && (e as { success: boolean | null }).success === null);
    expect(pendingEvents).toHaveLength(0);
  });
});

// ===========================================================================
// Phase 0: Systematic STATUS_TRANSITIONS coverage
// ===========================================================================
import { STATUS_TRANSITIONS, SECTION_TRANSITIONS, resolveActions, displaySection } from '../../generated/thread-lifecycle';

// Minimal event payloads for STATUS_TRANSITIONS testing — some event types
// require extra fields to avoid runtime errors.
const EVENT_PAYLOADS: Record<string, Record<string, unknown>> = {
  CodingAgentIdled: { has_changes: true, requires_restart: false, is_external_repo: false },
  ChangeProposed: { change_id: 'c-1' },
  ChangeApplied: { change_id: 'c-1' },
  ChangeDiscarded: { change_id: 'c-1' },
  ChangeApplyFailed: { change_id: 'c-1', error: 'err' },
  MergeConflictDetected: { change_id: 'c-1' },
  ThreadDismissed: {},
  ResponseGenerated: {},
  ResponseCanceled: {},
  ResponseAborted: {},
  ResponseFailed: { error: 'err' },
  SessionEnded: {},
  TriggerCompleted: { trigger_id: 't-1' },
  MessageReceived: { text: 'hi' },
  TriggerStarted: { trigger_id: 't-1' },
  CodingAgentUserMessageSent: { text: 'fix' },
  UserPromptInjected: { text: 'correction' },
  CodingAgentPromptSent: { text: 'prompt' },
};

/** Apply a single event to a fresh or pre-configured thread and return the resulting meta. */
function applyEvent(eventType: string, overrides: Partial<ThreadMeta> = {}): ThreadMeta {
  const thread = makeThreadState();
  Object.assign(thread.meta, overrides);
  const map = new Map([['thread-1', thread]]);
  const payload = { type: eventType, ...(EVENT_PAYLOADS[eventType] || {}) };
  handleEvent(map, 'thread-1', 1, payload as ThreadEvent, TS);
  return thread.meta;
}

describe('Phase 0 — systematic STATUS_TRANSITIONS coverage', () => {

  it('all STATUS_TRANSITIONS entries produce correct meta.status', () => {
    for (const [eventType, transition] of Object.entries(STATUS_TRANSITIONS)) {
      const rule = transition.status;

      if (rule.kind === 'set') {
        const meta = applyEvent(eventType, { status: 'idle' });
        expect(meta.status).toBe(rule.status);
      } else if (rule.kind === 'conditional_cc') {
        // Test with ccHasChanges = true
        const metaWith = applyEvent(eventType, { status: 'running', ccHasChanges: true });
        expect(metaWith.status).toBe(rule.withChanges);

        // Test with ccHasChanges = false
        if (eventType === 'CodingAgentIdled') {
          // CodingAgentIdled's from_payload rule overrides ccHasChanges from the event,
          // so we need a custom event with has_changes: false to test the withoutChanges path.
          const thread = makeThreadState();
          thread.meta.status = 'running';
          thread.meta.ccHasChanges = false;
          const map = new Map([['t', thread]]);
          handleEvent(map, 't', 1, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, TS);
          expect(thread.meta.status).toBe(rule.withoutChanges);
        } else {
          const metaWithout = applyEvent(eventType, { status: 'running', ccHasChanges: false });
          expect(metaWithout.status).toBe(rule.withoutChanges);
        }
      }
    }
  });

  it('ChangeProposed sets ccHasChanges', () => {
    const meta = applyEvent('ChangeProposed', { ccHasChanges: false });
    expect(meta.ccHasChanges).toBe(true);
  });

  it('ChangeApplied clears all CC flags', () => {
    const meta = applyEvent('ChangeApplied', {
      ccHasChanges: true, ccRequiresRestart: true, ccIsExternalRepo: true, ccApplying: true,
    });
    expect(meta.ccHasChanges).toBe(false);
    expect(meta.ccRequiresRestart).toBe(false);
    expect(meta.ccIsExternalRepo).toBe(false);
    expect(meta.ccApplying).toBe(false);
  });

  it('MergeConflictDetected sets ccApplying', () => {
    const meta = applyEvent('MergeConflictDetected', { ccApplying: false });
    expect(meta.ccApplying).toBe(true);
  });

  it('ChangeApplyFailed clears ccApplying only', () => {
    const meta = applyEvent('ChangeApplyFailed', { ccApplying: true, ccHasChanges: true });
    expect(meta.ccApplying).toBe(false);
    expect(meta.ccHasChanges).toBe(true); // other flags preserved
  });

  it('SECTION_TRANSITIONS entries update meta.section correctly', () => {
    for (const [eventType, expectedSection] of Object.entries(SECTION_TRANSITIONS)) {
      const startSection = expectedSection === 'default' ? 'unread' : 'default';
      const thread = makeThreadState();
      thread.meta.section = startSection as 'default' | 'unread';
      const map = new Map([['thread-1', thread]]);
      handleEvent(map, 'thread-1', 1, { type: eventType } as ThreadEvent, TS);
      expect(thread.meta.section).toBe(expectedSection);
    }
  });
});

// ===========================================================================
// ChangeApplied/ChangeDiscarded must NOT change section — Done button regression
// ===========================================================================
describe('ChangeApplied/ChangeDiscarded — section stays unread for Done button', () => {
  it('ChangeApplied does NOT change section from unread', () => {
    const thread = makeThreadState();
    thread.meta.section = 'unread';
    thread.meta.channel = 'claude_code';
    thread.meta.ccHasChanges = true;
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'ChangeApplied', change_id: 'c1' } as ThreadEvent, '2026-04-08T10:00:00Z');

    expect(thread.meta.section).toBe('unread');
  });

  it('ChangeDiscarded does NOT change section from unread', () => {
    const thread = makeThreadState();
    thread.meta.section = 'unread';
    thread.meta.channel = 'claude_code';
    thread.meta.ccHasChanges = true;
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'ChangeDiscarded', change_id: 'c1' } as ThreadEvent, '2026-04-08T10:00:00Z');

    expect(thread.meta.section).toBe('unread');
  });

  it('ChangeApplied clears all CC flags', () => {
    const meta = applyEvent('ChangeApplied', {
      ccHasChanges: true,
      ccRequiresRestart: true,
      ccIsExternalRepo: true,
      ccApplying: true,
    });
    expect(meta.ccHasChanges).toBe(false);
    expect(meta.ccRequiresRestart).toBe(false);
    expect(meta.ccIsExternalRepo).toBe(false);
    expect(meta.ccApplying).toBe(false);
  });

  it('ChangeDiscarded clears all CC flags', () => {
    const meta = applyEvent('ChangeDiscarded', {
      ccHasChanges: true,
      ccRequiresRestart: true,
      ccIsExternalRepo: true,
      ccApplying: true,
    });
    expect(meta.ccHasChanges).toBe(false);
    expect(meta.ccRequiresRestart).toBe(false);
    expect(meta.ccIsExternalRepo).toBe(false);
    expect(meta.ccApplying).toBe(false);
  });

  it('ChangeApplied sets status to idle', () => {
    const meta = applyEvent('ChangeApplied', { status: 'waiting' });
    expect(meta.status).toBe('idle');
  });

  it('ChangeDiscarded sets status to idle', () => {
    const meta = applyEvent('ChangeDiscarded', { status: 'waiting' });
    expect(meta.status).toBe('idle');
  });

  it('resolveActions returns done after apply (CC, idle, unread, no pending changes)', () => {
    const actions = resolveActions('claude_code', 'idle', 'unread', false);
    expect(actions).toEqual(['done']);
  });

  it('resolveActions returns empty after dismiss (CC, idle, default)', () => {
    const actions = resolveActions('claude_code', 'idle', 'default', false);
    expect(actions).toEqual([]);
  });

  it('displaySection returns review when unread + idle (after apply)', () => {
    const section = displaySection('unread', 'idle', false, false);
    expect(section).toBe('review');
  });

  it('displaySection returns history when default + idle (after dismiss)', () => {
    const section = displaySection('default', 'idle', false, false);
    expect(section).toBe('history');
  });

  it('ChangeApplied is NOT in SECTION_TRANSITIONS', () => {
    expect(SECTION_TRANSITIONS).not.toHaveProperty('ChangeApplied');
  });

  it('ChangeDiscarded is NOT in SECTION_TRANSITIONS', () => {
    expect(SECTION_TRANSITIONS).not.toHaveProperty('ChangeDiscarded');
  });

  it('full CC flow: idle with changes → apply → done button → dismiss → history', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t1', thread]]);

    // 1. User message starts thread
    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix bug' } as ThreadEvent, '2026-04-08T10:00:00Z');
    expect(thread.meta.status).toBe('running');

    // 2. CC session starts
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-08T10:00:01Z');

    // 3. CC proposes changes
    handleEvent(map, 't1', 3, { type: 'ChangeProposed', change_id: 'c1', files: ['a.rs'] } as ThreadEvent, '2026-04-08T10:00:02Z');
    expect(thread.meta.ccHasChanges).toBe(true);

    // 4. CC idles — backend emits CodingAgentIdled + ThreadMarkedUnread side-effect
    handleEvent(map, 't1', 4, { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, '2026-04-08T10:00:03Z');
    handleEvent(map, 't1', 5, { type: 'ThreadMarkedUnread' } as ThreadEvent, '2026-04-08T10:00:03Z');
    expect(thread.meta.section).toBe('unread');
    expect(thread.meta.ccHasChanges).toBe(true);
    expect(resolveActions('claude_code', thread.meta.status, thread.meta.section, thread.meta.ccHasChanges)).toEqual(['discard', 'apply']);
    expect(displaySection(thread.meta.section, thread.meta.status, false, false)).toBe('review');

    // 5. User clicks Apply — thread STAYS in REVIEW with Done button
    handleEvent(map, 't1', 6, { type: 'ChangeApplied', change_id: 'c1' } as ThreadEvent, '2026-04-08T10:00:04Z');
    expect(thread.meta.section).toBe('unread');
    expect(thread.meta.ccHasChanges).toBe(false);
    expect(thread.meta.status).toBe('idle');
    expect(resolveActions('claude_code', thread.meta.status, thread.meta.section, thread.meta.ccHasChanges)).toEqual(['done']);
    expect(displaySection(thread.meta.section, thread.meta.status, false, false)).toBe('review');

    // 6. User clicks Done — thread moves to HISTORY
    handleEvent(map, 't1', 7, { type: 'ThreadDismissed' } as ThreadEvent, '2026-04-08T10:00:05Z');
    expect(thread.meta.section).toBe('default');
    expect(resolveActions('claude_code', thread.meta.status, thread.meta.section, thread.meta.ccHasChanges)).toEqual([]);
    expect(displaySection(thread.meta.section, thread.meta.status, false, false)).toBe('history');
  });

  it('full CC flow: idle with changes → discard → done button → dismiss → history', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'fix bug' } as ThreadEvent, '2026-04-08T10:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-08T10:00:01Z');
    handleEvent(map, 't1', 3, { type: 'ChangeProposed', change_id: 'c1', files: ['a.rs'] } as ThreadEvent, '2026-04-08T10:00:02Z');
    handleEvent(map, 't1', 4, { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, '2026-04-08T10:00:03Z');
    // Backend side-effect: ThreadMarkedUnread
    handleEvent(map, 't1', 5, { type: 'ThreadMarkedUnread' } as ThreadEvent, '2026-04-08T10:00:03Z');

    // User clicks Discard — thread STAYS in REVIEW with Done button
    handleEvent(map, 't1', 6, { type: 'ChangeDiscarded', change_id: 'c1' } as ThreadEvent, '2026-04-08T10:00:04Z');
    expect(thread.meta.section).toBe('unread');
    expect(thread.meta.ccHasChanges).toBe(false);
    expect(thread.meta.status).toBe('idle');
    expect(resolveActions('claude_code', thread.meta.status, thread.meta.section, thread.meta.ccHasChanges)).toEqual(['done']);

    // User clicks Done — thread moves to HISTORY
    handleEvent(map, 't1', 7, { type: 'ThreadDismissed' } as ThreadEvent, '2026-04-08T10:00:05Z');
    expect(thread.meta.section).toBe('default');
    expect(displaySection(thread.meta.section, thread.meta.status, false, false)).toBe('history');
  });
});

// ===========================================================================
// exchangeStatus — CC follow-up scenarios (integration tests)
// ===========================================================================
// These tests simulate the full SSE→events→exchanges→status pipeline for CC
// follow-up messages. Each test builds events via handleEvent, groups them
// into exchanges, and asserts the user-visible status for each exchange.
//
// The bug: follow-up messages intermittently show "Aborted" status when the
// CC process exits during or after processing a follow-up.
// ===========================================================================

/** Helper: build a CC thread state and replay events through handleEvent,
 *  then return exchanges with their statuses. */
function buildCCThread(events: Array<{ seq: number; event: ThreadEvent; created: string }>): {
  thread: ThreadState;
  exchanges: ReturnType<typeof groupIntoExchanges>;
  statuses: ReturnType<typeof exchangeStatus>[];
} {
  const thread = makeThreadState();
  thread.meta.channel = 'claude_code';
  const map = new Map([['t', thread]]);

  for (const { seq, event, created } of events) {
    handleEvent(map, 't', seq, event, created);
  }

  const exchanges = groupIntoExchanges(thread.events);
  const statuses = exchanges.map((exch, i) =>
    exchangeStatus(exch, '', i === exchanges.length - 1, false, true)
  );
  return { thread, exchanges, statuses };
}

describe('exchangeStatus — CC follow-up happy path', () => {
  it('normal CC session: message → work → idle = done', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      { seq: 5, event: { type: 'CodingAgentTextStreamed', text: 'Fixed!' } as ThreadEvent, created: '2026-04-12T10:00:04Z' },
      { seq: 6, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:05Z' },
    ]);
    expect(statuses).toHaveLength(1);
    expect(statuses[0]).toBe('done');
  });

  it('CC follow-up: idle → follow-up message → CC resumes → idle = both done', () => {
    const { statuses, exchanges } = buildCCThread([
      // Exchange 1: initial request
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      { seq: 5, event: { type: 'CodingAgentTextStreamed', text: 'Done with initial analysis' } as ThreadEvent, created: '2026-04-12T10:00:04Z' },
      { seq: 6, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:05Z' },
      // Exchange 2: follow-up
      { seq: 7, event: { type: 'MessageReceived', text: 'now also fix the tests', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 8, event: { type: 'CodingAgentUserMessageSent', text: 'now also fix the tests' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 9, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 10, event: { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
      { seq: 11, event: { type: 'CodingAgentTextStreamed', text: 'Tests fixed!' } as ThreadEvent, created: '2026-04-12T10:01:03Z' },
      { seq: 12, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:01:04Z' },
    ]);
    expect(exchanges).toHaveLength(2);
    expect(statuses[0]).toBe('done');      // Exchange 1: completed normally
    expect(statuses[1]).toBe('done');      // Exchange 2: follow-up completed normally
  });

  it('CC follow-up without CodingAgentUserMessageSent (new data path) = both done', () => {
    const { statuses, exchanges } = buildCCThread([
      // Exchange 1
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:05Z' },
      // Exchange 2: follow-up with only MessageReceived (no CodingAgentUserMessageSent)
      { seq: 4, event: { type: 'MessageReceived', text: 'also fix tests', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 6, event: { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
      { seq: 7, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:01:03Z' },
    ]);
    expect(exchanges).toHaveLength(2);
    expect(statuses[0]).toBe('done');
    expect(statuses[1]).toBe('done');
  });

  it('multiple follow-ups all complete normally = all done', () => {
    const { statuses, exchanges } = buildCCThread([
      // Exchange 1
      { seq: 1, event: { type: 'MessageReceived', text: 'first', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Exchange 2
      { seq: 4, event: { type: 'MessageReceived', text: 'second', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentUserMessageSent', text: 'second' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 6, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
      // Exchange 3
      { seq: 7, event: { type: 'MessageReceived', text: 'third', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:02:00Z' },
      { seq: 8, event: { type: 'CodingAgentUserMessageSent', text: 'third' } as ThreadEvent, created: '2026-04-12T10:02:00Z' },
      { seq: 9, event: { type: 'CodingAgentTextStreamed', text: 'All done!' } as ThreadEvent, created: '2026-04-12T10:02:01Z' },
      { seq: 10, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:02:02Z' },
    ]);
    expect(exchanges).toHaveLength(3);
    expect(statuses[0]).toBe('done');
    expect(statuses[1]).toBe('done');
    expect(statuses[2]).toBe('done');
  });
});

describe('exchangeStatus — CC follow-up abort scenarios', () => {
  it('CC process crash mid-follow-up: ResponseAborted = aborted (not done)', () => {
    const { statuses, exchanges } = buildCCThread([
      // Exchange 1: completes normally
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Exchange 2: follow-up — CC crashes mid-work
      { seq: 4, event: { type: 'MessageReceived', text: 'also fix tests', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentUserMessageSent', text: 'also fix tests' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 6, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      // CC process dies — safety-net ResponseAborted
      { seq: 7, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
    ]);
    expect(exchanges).toHaveLength(2);
    expect(statuses[0]).toBe('done');      // Exchange 1 was already done
    expect(statuses[1]).toBe('aborted');   // Exchange 2 aborted by crash
  });

  it('CC crash on follow-up is NOT flagged as engine restart', () => {
    const { exchanges } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
    ]);
    // CC crash — should say "interrupted", NOT "engine restarted"
    expect(isAbortedByRestart(exchanges[1])).toBe(false);
  });

  it('engine restart on follow-up IS flagged as engine restart', () => {
    const { exchanges, statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'SessionEnded', reason: 'shutdown' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
    ]);
    expect(statuses[1]).toBe('aborted');
    expect(isAbortedByRestart(exchanges[1])).toBe(true);
  });

  it('lost follow-up during CC exit: ResponseAborted for drained messages = aborted', () => {
    const { statuses, exchanges } = buildCCThread([
      // Exchange 1: normal
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Exchange 2: follow-up sent, but CC was exiting — message lost
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up that got lost', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      // Backend drains lost follow-ups → single ResponseAborted
      { seq: 5, event: { type: 'ResponseAborted', text: '1 follow-up message(s) lost during session exit' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
    ]);
    expect(exchanges).toHaveLength(2);
    expect(statuses[0]).toBe('done');
    expect(statuses[1]).toBe('aborted');
  });

  it.each([
    'completed', 'user_ended', 'changes_proposed', 'auto_ended', 'discarded', 'changes_applied',
  ] as const)('follow-up with SessionEnded(%s) after idle = done, NOT aborted', (reason) => {
    const { statuses, exchanges } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 6, event: { type: 'SessionEnded', reason } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
    ]);
    expect(exchanges).toHaveLength(2);
    expect(statuses[0]).toBe('done');
    expect(statuses[1]).toBe('done');
  });

  it('follow-up with ResponseAborted + SessionEnded(completed) mid-work = aborted', () => {
    // Engine flow when CC dies before producing a Result for a follow-up:
    // the run_session safety net emits ResponseAborted, then the post-loop
    // emits SessionEnded(completed). The exchange reads as aborted because
    // ResponseAborted set isAborted=true; SessionEnded(completed) is just
    // the normal lifecycle terminator that follows.
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Follow-up — CC starts working, dies without Result, safety net fires
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 6, event: { type: 'ResponseAborted' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
      { seq: 7, event: { type: 'SessionEnded', reason: 'completed' } as ThreadEvent, created: '2026-04-12T10:01:03Z' },
    ]);
    expect(statuses[1]).toBe('aborted');
  });
});

describe('exchangeStatus — CC follow-up in-progress states', () => {
  it('follow-up with no response events yet = pending', () => {
    const { statuses, exchanges } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Follow-up sent, but no CC response events yet
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
    ]);
    expect(exchanges).toHaveLength(2);
    expect(statuses[0]).toBe('done');
    expect(statuses[1]).toBe('pending');
  });

  it('follow-up with CC tool calls in progress = cc-working', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Follow-up — CC starts working
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
    ]);
    expect(statuses[1]).toBe('cc-working');
  });

  it('follow-up mid-streaming (text arrived, no completion) = cc-working', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentTextStreamed', text: 'Working on it...' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
    ]);
    expect(statuses[1]).toBe('cc-working');
  });
});

describe('exchangeStatus — CC follow-up edge cases', () => {
  it('ResponseAborted on exchange 1 does NOT infect exchange 2', () => {
    const { statuses, exchanges } = buildCCThread([
      // Exchange 1: aborted
      { seq: 1, event: { type: 'MessageReceived', text: 'first', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Exchange 2: new session starts fresh
      { seq: 4, event: { type: 'MessageReceived', text: 'try again', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'SessionStarted', session_id: 's2' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 6, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
    ]);
    expect(exchanges).toHaveLength(2);
    expect(statuses[0]).toBe('aborted');
    expect(statuses[1]).toBe('done');
  });

  it('CC resumes after idle then completes = done (idle → work → idle)', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      // CC idles then self-resumes (e.g., hardening follow-up)
      { seq: 5, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:04Z' },
      { seq: 6, event: { type: 'CodingAgentPromptSent', text: 'Run /harden now.' } as ThreadEvent, created: '2026-04-12T10:00:05Z' },
      { seq: 7, event: { type: 'CodingAgentToolCalled', name: 'Bash', args: {} } as ThreadEvent, created: '2026-04-12T10:00:06Z' },
      { seq: 8, event: { type: 'CodingAgentToolResult', name: 'Bash', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:00:07Z' },
      { seq: 9, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:00:08Z' },
    ]);
    expect(statuses).toHaveLength(1);
    expect(statuses[0]).toBe('done');
  });

  it('follow-up sent while CC is actively working (not idle) = cc-working for exchange 1', () => {
    // This tests the scenario where exchange 1 has CC activity, then a follow-up
    // creates exchange 2. Exchange 1 should be 'interrupted' since it had steps
    // but no completion event, and exchange 2 should be the active one.
    const { statuses, exchanges } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Follow-up arrives mid-work (before first exchange completes)
      { seq: 4, event: { type: 'MessageReceived', text: 'actually do this instead', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      { seq: 5, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:00:04Z' },
      { seq: 6, event: { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:00:05Z' },
      { seq: 7, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:06Z' },
    ]);
    expect(exchanges).toHaveLength(2);
    expect(statuses[0]).toBe('interrupted'); // Had steps but no completion
    expect(statuses[1]).toBe('done');
  });

  it('follow-up with ResponseGenerated (chat-style completion in CC thread) = done', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentTextStreamed', text: 'Here you go' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 6, event: { type: 'ResponseGenerated', text: 'Here you go' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
    ]);
    expect(statuses[1]).toBe('done');
  });

  it('follow-up with ResponseCanceled = canceled, NOT aborted', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 6, event: { type: 'ResponseCanceled' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
    ]);
    expect(statuses[1]).toBe('canceled');
  });

  it('follow-up with ResponseFailed = error, NOT aborted', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'ResponseFailed', error: 'API timeout' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
    ]);
    expect(statuses[1]).toBe('error');
  });
});

describe('exchangeStatus — CC follow-up with pending user messages (optimistic)', () => {
  it('optimistic follow-up before SSE confirmation = pending (not aborted)', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t', thread]]);

    // Exchange 1: completed
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, '2026-04-12T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-12T10:00:01Z');
    handleEvent(map, 't', 3, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, '2026-04-12T10:00:02Z');

    // Optimistic follow-up (pending, not yet confirmed by SSE)
    thread.pendingUserMessages.push({
      text: 'now also fix the tests',
      eventId: 'msg-optimistic-1',
      created: '2026-04-12T10:01:00Z',
    });

    const exchanges = computeExchanges(thread);
    expect(exchanges).toHaveLength(2);

    const status0 = exchangeStatus(exchanges[0], '', false, false, true);
    const status1 = exchangeStatus(exchanges[1], '', true, false, true);
    expect(status0).toBe('done');
    expect(status1).toBe('pending'); // Optimistic — not aborted
  });

  it('optimistic follow-up resolved by SSE MessageReceived = normal flow', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t', thread]]);

    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, '2026-04-12T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-12T10:00:01Z');
    handleEvent(map, 't', 3, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, '2026-04-12T10:00:02Z');

    // Add optimistic
    thread.pendingUserMessages.push({
      text: 'follow-up',
      eventId: 'msg-1',
      created: '2026-04-12T10:01:00Z',
    });

    // SSE confirms the message — pending clears, real events arrive
    handleEvent(map, 't', 4, { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, '2026-04-12T10:01:00Z', 'msg-1');
    handleEvent(map, 't', 5, { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, '2026-04-12T10:01:01Z');
    handleEvent(map, 't', 6, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' } as ThreadEvent, '2026-04-12T10:01:02Z');
    handleEvent(map, 't', 7, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, '2026-04-12T10:01:03Z');

    expect(thread.pendingUserMessages).toHaveLength(0);
    const exchanges = computeExchanges(thread);
    expect(exchanges).toHaveLength(2);

    const status1 = exchangeStatus(exchanges[1], '', true, false, true);
    expect(status1).toBe('done');
  });
});

describe('exchangeStatus — CC session recovery and restart', () => {
  it('SessionRecovered exchange after restart = done (not aborted)', () => {
    const { statuses, exchanges } = buildCCThread([
      // Exchange 1: original session, engine restarted
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'SessionEnded', reason: 'shutdown' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      // Exchange 2: recovery after restart
      { seq: 5, event: { type: 'SessionRecovered', branch: 'claude-code/fix' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 6, event: { type: 'SessionStarted', session_id: 's2' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 7, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
    ]);
    expect(exchanges).toHaveLength(2);
    expect(statuses[0]).toBe('aborted');  // Original was aborted by shutdown
    expect(statuses[1]).toBe('done');     // Recovery completed fine
  });

  it('isAbortedByRestart correctly identifies shutdown in multi-event exchange', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix', channel: 'claude_code', created: '2026-04-12T10:00:00Z' } as ThreadEvent],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-04-12T10:00:01Z' } as ThreadEvent],
      [3, { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: '2026-04-12T10:00:02Z' } as ThreadEvent],
      [4, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: '2026-04-12T10:00:03Z' } as ThreadEvent],
      [5, { type: 'CodingAgentTextStreamed', text: 'Working...', created: '2026-04-12T10:00:04Z' } as ThreadEvent],
      [6, { type: 'SessionEnded', reason: 'shutdown', created: '2026-04-12T10:00:05Z' } as ThreadEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(isAbortedByRestart(exchanges[0])).toBe(true);
  });

  it('isAbortedByRestart returns false for SessionEnded with normal reasons', () => {
    const normalReasons: Array<string | undefined> = ['completed', 'user_ended', 'changes_proposed', 'changes_applied', 'auto_ended', 'discarded', undefined];

    for (const reason of normalReasons) {
      const events = new Map<number, ThreadEvent>([
        [1, { type: 'MessageReceived', text: 'fix', channel: 'claude_code', created: '2026-04-12T10:00:00Z' } as ThreadEvent],
        [2, { type: 'SessionStarted', session_id: 's1', created: '2026-04-12T10:00:01Z' } as ThreadEvent],
        [3, { type: 'CodingAgentIdled', has_changes: false, created: '2026-04-12T10:00:02Z' } as ThreadEvent],
        [4, { type: 'SessionEnded', reason, created: '2026-04-12T10:00:03Z' } as ThreadEvent],
      ]);
      const exchanges = groupIntoExchanges(events);
      expect(isAbortedByRestart(exchanges[0])).toBe(false);
    }
  });

  it('isAbortedByRestart returns false for ResponseAborted without SessionEnded', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix', channel: 'claude_code', created: '2026-04-12T10:00:00Z' } as ThreadEvent],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-04-12T10:00:01Z' } as ThreadEvent],
      [3, { type: 'ResponseAborted', text: 'crash', created: '2026-04-12T10:00:02Z' } as ThreadEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(isAbortedByRestart(exchanges[0])).toBe(false);
  });
});

describe('exchangeStatus — CC follow-up grouping correctness', () => {
  it('CodingAgentUserMessageSent dedupes with preceding MessageReceived', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix', channel: 'claude_code', created: '2026-04-12T10:00:00Z' } as ThreadEvent],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-04-12T10:00:01Z' } as ThreadEvent],
      [3, { type: 'CodingAgentIdled', has_changes: false, created: '2026-04-12T10:00:02Z' } as ThreadEvent],
      // Follow-up: both MessageReceived and CodingAgentUserMessageSent
      [4, { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code', created: '2026-04-12T10:01:00Z' } as ThreadEvent],
      [5, { type: 'CodingAgentUserMessageSent', text: 'follow-up', created: '2026-04-12T10:01:00Z' } as ThreadEvent],
      [6, { type: 'CodingAgentIdled', has_changes: false, created: '2026-04-12T10:01:01Z' } as ThreadEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    // Must NOT create 3 exchanges — CodingAgentUserMessageSent should be deduped
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('MessageReceived');
  });

  it('legacy CodingAgentUserMessageSent without MessageReceived creates exchange', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix', channel: 'claude_code', created: '2026-04-12T10:00:00Z' } as ThreadEvent],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-04-12T10:00:01Z' } as ThreadEvent],
      [3, { type: 'CodingAgentIdled', has_changes: false, created: '2026-04-12T10:00:02Z' } as ThreadEvent],
      // Legacy path: only CodingAgentUserMessageSent, no MessageReceived
      [4, { type: 'CodingAgentUserMessageSent', text: 'follow-up', created: '2026-04-12T10:01:00Z' } as ThreadEvent],
      [5, { type: 'CodingAgentIdled', has_changes: false, created: '2026-04-12T10:01:01Z' } as ThreadEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    // Legacy path creates synthetic MessageReceived
    expect(exchanges[1].userEvent.type).toBe('MessageReceived');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('follow-up');
  });

  it('events between exchanges are attributed to the correct exchange', () => {
    const { exchanges } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'first', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      { seq: 5, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:04Z' },
      // Follow-up
      { seq: 6, event: { type: 'MessageReceived', text: 'second', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 7, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 8, event: { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
      { seq: 9, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:01:03Z' },
    ]);
    // Exchange 1: SessionStarted, Read call/result, Idled
    expect(exchanges[0].steps).toHaveLength(4); // SessionStarted + ToolCalled + ToolResult + Idled
    // Exchange 2: ToolCalled + ToolResult + Idled
    expect(exchanges[1].steps).toHaveLength(3);
  });
});

describe('exchangeStatus — non-last exchange positioning', () => {
  it('completed non-last exchange = done (not interrupted when has completion event)', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t', thread]]);

    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'first', channel: 'claude_code' } as ThreadEvent, '2026-04-12T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-12T10:00:01Z');
    handleEvent(map, 't', 3, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, '2026-04-12T10:00:02Z');
    handleEvent(map, 't', 4, { type: 'MessageReceived', text: 'second', channel: 'claude_code' } as ThreadEvent, '2026-04-12T10:01:00Z');
    handleEvent(map, 't', 5, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, '2026-04-12T10:01:01Z');
    handleEvent(map, 't', 6, { type: 'MessageReceived', text: 'third', channel: 'claude_code' } as ThreadEvent, '2026-04-12T10:02:00Z');
    handleEvent(map, 't', 7, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, '2026-04-12T10:02:01Z');

    const exchanges = groupIntoExchanges(thread.events);
    expect(exchanges).toHaveLength(3);

    // Test each position explicitly
    const s0 = exchangeStatus(exchanges[0], '', false, false, true);
    const s1 = exchangeStatus(exchanges[1], '', false, false, true);
    const s2 = exchangeStatus(exchanges[2], '', true, false, true);
    expect(s0).toBe('done');
    expect(s1).toBe('done');
    expect(s2).toBe('done');
  });

  it('non-last CC exchange with steps but no completion = interrupted', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t', thread]]);

    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'first', channel: 'claude_code' } as ThreadEvent, '2026-04-12T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-12T10:00:01Z');
    handleEvent(map, 't', 3, { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, '2026-04-12T10:00:02Z');
    // No completion — follow-up arrives
    handleEvent(map, 't', 4, { type: 'MessageReceived', text: 'second', channel: 'claude_code' } as ThreadEvent, '2026-04-12T10:01:00Z');
    handleEvent(map, 't', 5, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, '2026-04-12T10:01:01Z');

    const exchanges = groupIntoExchanges(thread.events);
    const s0 = exchangeStatus(exchanges[0], '', false, false, true);
    const s1 = exchangeStatus(exchanges[1], '', true, false, true);
    expect(s0).toBe('interrupted');
    expect(s1).toBe('done');
  });
});

describe('exchangeStatus — auto-harden crash must not contaminate exchange', () => {
  it('follow-up completed (ResponseGenerated) then auto-harden crash = done, NOT aborted', () => {
    // This is the exact scenario causing the intermittent "Aborted" on follow-ups:
    // 1. Follow-up sent → CC works → ResponseGenerated (user work done)
    // 2. Auto-harden injected → CodingAgentPromptSent
    // 3. CC crashes during hardening → ResponseAborted
    // The user's work was completed. The harden crash is system-level.
    const { statuses, exchanges } = buildCCThread([
      // Exchange 1: initial request
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Exchange 2: follow-up
      { seq: 4, event: { type: 'MessageReceived', text: 'now fix tests', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentUserMessageSent', text: 'now fix tests' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 6, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 7, event: { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
      { seq: 8, event: { type: 'CodingAgentTextStreamed', text: 'Tests fixed!' } as ThreadEvent, created: '2026-04-12T10:01:03Z' },
      // CC completes user work (ResponseGenerated from Result event)
      { seq: 9, event: { type: 'ResponseGenerated', text: 'Tests fixed!' } as ThreadEvent, created: '2026-04-12T10:01:04Z' },
      // Auto-harden kicks in (system-injected prompt)
      { seq: 10, event: { type: 'CodingAgentPromptSent', text: 'Run /harden now.' } as ThreadEvent, created: '2026-04-12T10:01:05Z' },
      { seq: 11, event: { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'cargo test' } } as ThreadEvent, created: '2026-04-12T10:01:06Z' },
      // CC crashes during hardening
      { seq: 12, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:01:07Z' },
    ]);
    expect(exchanges).toHaveLength(2);
    expect(statuses[0]).toBe('done');
    expect(statuses[1]).toBe('done'); // User work was done — harden crash is system-level
  });

  it('follow-up completed (CodingAgentIdled) then auto-harden crash = done, NOT aborted', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Follow-up
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentTextStreamed', text: 'Done!' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      // CC idles (user work complete) but auto-harden has NOT yet fired
      // Note: in the real code, auto-harden fires BEFORE CodingAgentIdled.
      // But if the harden marker IS fresh, CodingAgentIdled fires normally.
      { seq: 6, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
      // Then the engine's auto-harden retriggers (marker became stale after commit)
      { seq: 7, event: { type: 'CodingAgentPromptSent', text: 'Run /harden now.' } as ThreadEvent, created: '2026-04-12T10:01:03Z' },
      { seq: 8, event: { type: 'CodingAgentToolCalled', name: 'Bash', args: {} } as ThreadEvent, created: '2026-04-12T10:01:04Z' },
      { seq: 9, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:01:05Z' },
    ]);
    expect(statuses[1]).toBe('done'); // User work was done, harden crash is system-level
  });

  it('initial exchange completed then system prompt crash = done, NOT aborted', () => {
    // Same scenario but for the initial exchange (not a follow-up)
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      // CC completes, ResponseGenerated emitted
      { seq: 5, event: { type: 'ResponseGenerated', text: 'Fixed!' } as ThreadEvent, created: '2026-04-12T10:00:04Z' },
      // Auto-harden injected
      { seq: 6, event: { type: 'CodingAgentPromptSent', text: 'Run /harden now.' } as ThreadEvent, created: '2026-04-12T10:00:05Z' },
      { seq: 7, event: { type: 'CodingAgentToolCalled', name: 'Bash', args: {} } as ThreadEvent, created: '2026-04-12T10:00:06Z' },
      // Harden crash
      { seq: 8, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:00:07Z' },
    ]);
    expect(statuses[0]).toBe('done');
  });

  it('genuine crash before any completion = aborted (not affected by fix)', () => {
    // Ensure the fix doesn't accidentally suppress real aborts
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // CC crashes before completing — no ResponseGenerated or CodingAgentIdled
      { seq: 4, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
    ]);
    expect(statuses[0]).toBe('aborted'); // Genuine crash — must stay aborted
  });

  it('CC crash during follow-up before any completion = aborted (genuine crash)', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Follow-up — CC crashes mid-work, never completed
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 6, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
    ]);
    // Exchange 2 never completed — genuine crash
    expect(statuses[1]).toBe('aborted');
  });

  it('shutdown after CodingAgentIdled: exchange was complete = done, NOT aborted', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      { seq: 5, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:00:04Z' },
      // Engine shuts down while CC was idle
      { seq: 6, event: { type: 'SessionEnded', reason: 'shutdown' } as ThreadEvent, created: '2026-04-12T10:00:05Z' },
    ]);
    // CC was idle — work was done. Shutdown doesn't undo that.
    expect(statuses[0]).toBe('done');
  });
});

describe('exchangeStatus — chat thread follow-up (non-CC)', () => {
  it('chat follow-up with ResponseAborted = aborted', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'chat';
    const map = new Map([['t', thread]]);

    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'hello' } as ThreadEvent, '2026-04-12T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'TextStreamed', text: 'Hi!' } as ThreadEvent, '2026-04-12T10:00:01Z');
    handleEvent(map, 't', 3, { type: 'ResponseGenerated' } as ThreadEvent, '2026-04-12T10:00:02Z');
    handleEvent(map, 't', 4, { type: 'MessageReceived', text: 'tell me more' } as ThreadEvent, '2026-04-12T10:01:00Z');
    handleEvent(map, 't', 5, { type: 'ResponseAborted' } as ThreadEvent, '2026-04-12T10:01:01Z');

    const exchanges = groupIntoExchanges(thread.events);
    const s0 = exchangeStatus(exchanges[0], '', false, false, false);
    const s1 = exchangeStatus(exchanges[1], '', true, false, false);
    expect(s0).toBe('done');
    expect(s1).toBe('aborted');
  });

  it('chat follow-up normal completion = done', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'chat';
    const map = new Map([['t', thread]]);

    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'hello' } as ThreadEvent, '2026-04-12T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'ResponseGenerated', text: 'Hi!' } as ThreadEvent, '2026-04-12T10:00:01Z');
    handleEvent(map, 't', 3, { type: 'MessageReceived', text: 'follow-up' } as ThreadEvent, '2026-04-12T10:01:00Z');
    handleEvent(map, 't', 4, { type: 'ResponseGenerated', text: 'Sure!' } as ThreadEvent, '2026-04-12T10:01:01Z');

    const exchanges = groupIntoExchanges(thread.events);
    const s0 = exchangeStatus(exchanges[0], '', false, false, false);
    const s1 = exchangeStatus(exchanges[1], '', true, false, false);
    expect(s0).toBe('done');
    expect(s1).toBe('done');
  });
});

// ===========================================================================
// Phase 4 — terminal-only SessionEnded semantics
// ===========================================================================
// Under the new model, SessionEnded fires only for terminal reasons
// ('shutdown', 'panic', 'closed', 'legacy_non_terminal'). Turn boundaries
// (CodingAgentIdled, ChangeProposed, ResponseCanceled) leave the thread
// alive and ready to receive more turns. These tests pin that contract on
// the frontend so any regression in the status machine surfaces here.
//
// "Active" = thread can resume — meta.status is not 'failed', the thread
// is still in the map, and a subsequent MessageReceived transitions it
// back to 'running' (the running-after-Idled assertion is the load-bearing
// one: it proves the thread wasn't torn down by the prior turn).
// ===========================================================================
describe('Phase 4 — thread lifecycle under terminal-only SessionEnded', () => {
  it('thread is active after CodingAgentIdled', () => {
    // CodingAgentIdled is now a turn boundary, not a thread terminator.
    // The thread must remain alive: a follow-up message should bring it
    // back to 'running' without any SessionEnded in between.
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'first turn' } as ThreadEvent, '2026-04-24T10:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-24T10:00:01Z');
    handleEvent(map, 't1', 3, { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, '2026-04-24T10:00:02Z');
    handleEvent(map, 't1', 4, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, '2026-04-24T10:00:03Z');

    // Idled with no changes → status 'idle' (turn done) but thread is still
    // alive: not 'failed', still in the map, and ready to resume.
    expect(thread.meta.status).toBe('idle');
    expect(map.has('t1')).toBe(true);

    // Follow-up turn brings it back to 'running' — proves the thread wasn't
    // closed by the previous CodingAgentIdled.
    handleEvent(map, 't1', 5, { type: 'MessageReceived', text: 'second turn' } as ThreadEvent, '2026-04-24T10:00:10Z');
    expect(thread.meta.status).toBe('running');
  });

  it('thread is active after ChangeProposed', () => {
    // ChangeProposed now fires per commit and does not terminate the thread.
    // The status rule is 'no_change' — only ccHasChanges flips on. Multiple
    // ChangeProposed events for the same branch must accumulate without
    // overwriting each other (they live in the events Map keyed by seq).
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    thread.meta.status = 'running';
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'do work' } as ThreadEvent, '2026-04-24T10:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-24T10:00:01Z');
    // Three commits in the same turn — emitted per-commit by the post-commit hook.
    handleEvent(map, 't1', 3, { type: 'ChangeProposed', change_id: 'c1', commit_sha: 'aaa111', description: 'first' } as ThreadEvent, '2026-04-24T10:00:02Z');
    handleEvent(map, 't1', 4, { type: 'ChangeProposed', change_id: 'c1', commit_sha: 'bbb222', description: 'second' } as ThreadEvent, '2026-04-24T10:00:03Z');
    handleEvent(map, 't1', 5, { type: 'ChangeProposed', change_id: 'c1', commit_sha: 'ccc333', description: 'third' } as ThreadEvent, '2026-04-24T10:00:04Z');

    // Status is unchanged from 'running' (ChangeProposed has 'no_change' rule),
    // ccHasChanges flips on, all three events are preserved (no overwrites).
    expect(thread.meta.status).toBe('running');
    expect(thread.meta.ccHasChanges).toBe(true);
    const proposed = [...thread.events.values()].filter(e => e.type === 'ChangeProposed');
    expect(proposed).toHaveLength(3);
    expect(proposed.map(e => (e as { commit_sha?: string }).commit_sha)).toEqual(['aaa111', 'bbb222', 'ccc333']);
    expect(map.has('t1')).toBe(true);
  });

  it('thread is active after ResponseCanceled', () => {
    // Cancel is a turn boundary, not a thread end. The thread must stay
    // alive so the user can immediately type a follow-up.
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'long task' } as ThreadEvent, '2026-04-24T10:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-24T10:00:01Z');
    handleEvent(map, 't1', 3, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} } as ThreadEvent, '2026-04-24T10:00:02Z');
    handleEvent(map, 't1', 4, { type: 'ResponseCanceled' } as ThreadEvent, '2026-04-24T10:00:03Z');

    // ResponseCanceled drops to 'idle' (no changes) but thread is alive,
    // not 'failed', and resumable.
    expect(thread.meta.status).toBe('idle');
    expect(map.has('t1')).toBe(true);

    // Follow-up brings it back to 'running'.
    handleEvent(map, 't1', 5, { type: 'MessageReceived', text: 'try again' } as ThreadEvent, '2026-04-24T10:00:10Z');
    expect(thread.meta.status).toBe('running');
  });

  it('thread is closed only on SessionEnded with terminal reason', () => {
    // SessionEnded is now reserved for genuine terminal events (shutdown,
    // panic, closed). The exchange surfaces this as 'aborted' when the
    // session was killed mid-work (no CodingAgentIdled before the end).
    // Compared to the previous three tests, this is the only path where the
    // user's CC session was actually killed by the engine — every other
    // turn boundary leaves the session alive for the next prompt.
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t1', thread]]);

    handleEvent(map, 't1', 1, { type: 'MessageReceived', text: 'do work' } as ThreadEvent, '2026-04-24T10:00:00Z');
    handleEvent(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-24T10:00:01Z');
    handleEvent(map, 't1', 3, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, '2026-04-24T10:00:02Z');
    handleEvent(map, 't1', 4, { type: 'SessionEnded', reason: 'shutdown' } as ThreadEvent, '2026-04-24T10:00:03Z');

    // Mid-work shutdown — exchange shows 'aborted', distinguishing it from
    // the active-after-Idled / -Canceled / -ChangeProposed cases above.
    const exchanges = groupIntoExchanges(thread.events);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true, false, true)).toBe('aborted');
  });
});

