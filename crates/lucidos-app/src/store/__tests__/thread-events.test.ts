import { describe, it, expect } from 'vitest';
import {
  groupIntoExchanges,
  handleEvent,
  exchangeResponseEvents,
  exchangeSteps,
  exchangeStatus,
  computeExchanges,
  unresumedAbortIndex,
  resumeEngineNote,
  fullCommandForEngineTool,
  type ThreadAggregate,
  type ThreadEvent,
  type StoredEvent,
  type TransientEvent,
  type ThreadState,
  type ThreadMeta,
} from '../thread-events';
import { handleEventWithAgg } from './aggregate-test-helper';

const TS = '2026-04-17T00:00:00Z';

function makeThreadState(events: Map<number, ThreadEvent> = new Map()): ThreadState {
  const meta: ThreadMeta = {
    id: 'thread-1',
    title: 'Test Thread',
    channel: 'chat',
    initiator: 'user',
    saved: false,
    createdAt: '2026-01-01T00:00:00.000Z',
    updatedAt: '2026-01-01T00:00:00.000Z',
    status: 'idle',
    ccHasChanges: false,
    ccRequiresRestart: false,
    ccIsExternalRepo: false,
    ccApplying: false,
    lastRevivedAt: '',
    messageCount: 0,
    section: 'archived',
    activeChildrenCount: 0,
    totalChildrenCount: 0,
    state: 'active',
  };
  return { meta, events, streamingBuffer: '', eventsLoaded: true, eventsLoadFailed: false, lastDbSeq: 0, pendingUserMessages: [] };
}

// ===========================================================================
// Aggregate snapshot supersedes per-event SECTION_TRANSITIONS / STATUS_TRANSITIONS
// ===========================================================================
describe('aggregate-takes-precedence over event-type lookups', () => {
  function makeAggregate(overrides: Partial<ThreadAggregate> = {}): ThreadAggregate {
    return {
      threadId: 'thread-1',
      title: 'Test Thread',
      channel: 'chat',
      initiator: 'user',
      createdAt: '2026-01-01T00:00:00Z',
      lastActivity: '2026-04-17T00:00:00Z',
      messageCount: 1,
      section: 'archived',
      status: 'idle',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      isSaved: false,
      hasResponse: true,
      lastRevivedAt: null,
      parentThreadId: null,
      parentThreadTitle: null,
      state: 'active',
      ...overrides,
    };
  }

  it('aggregate.section overrides what SECTION_TRANSITIONS would have set', () => {
    const thread = makeThreadState();
    thread.meta.section = 'inbox';
    const map = new Map([['thread-1', thread]]);
    // ResponseCanceled would normally set section='inbox' via the lookup —
    // aggregate says 'archived' so aggregate wins.
    handleEvent(
      map,
      'thread-1',
      5,
      { type: 'ResponseCanceled', text: '', images: [] } as ThreadEvent,
      TS,
      'evt-5',
      makeAggregate({ section: 'archived' }),
    );
    expect(thread.meta.section).toBe('archived');
  });

  it('aggregate.status overrides updateStatusFromEvent', () => {
    const thread = makeThreadState();
    thread.meta.status = 'running';
    const map = new Map([['thread-1', thread]]);
    // ResponseGenerated with no CC changes would normally drive status='idle' —
    // aggregate says 'waiting' (e.g. cc_has_changes was set in the same exchange).
    handleEvent(
      map,
      'thread-1',
      5,
      { type: 'ResponseGenerated' } as ThreadEvent,
      TS,
      'evt-5',
      makeAggregate({ status: 'waiting' }),
    );
    expect(thread.meta.status).toBe('waiting');
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

  it('splits at ResponseAborted: terminates prior exchange AND opens an empty boundary exchange', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix the bug' }],
      [2, { type: 'TextStreamed', text: 'Working...' }],
      [3, { type: 'ResponseAborted', text: 'Working...' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    // Prior exchange keeps the abort as its terminating step (drives 'aborted' status)
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual(['TextStreamed', 'ResponseAborted']);
    // New boundary exchange wraps the AbortPanel
    expect(exchanges[1].userEvent.type).toBe('ResponseAborted');
    expect(exchanges[1].steps).toHaveLength(0);
  });

  it('splits at SessionRecovered: opens a resume exchange that absorbs the engine note', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix the bug' }],
      [2, { type: 'ResponseAborted', text: '' }],
      [3, { type: 'SessionRecovered' }],
      // Engine-mode UserPromptInjected (the engine note) — must absorb into the
      // SessionRecovered exchange as a step, not start a new exchange.
      [4, { type: 'UserPromptInjected', text: '[Engine note]', mode: 'engine' }],
      [5, { type: 'TextStreamed', text: 'On it.' }],
      [6, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[1].userEvent.type).toBe('ResponseAborted'); // boundary
    expect(exchanges[2].userEvent.type).toBe('SessionRecovered');
    expect(exchanges[2].steps.map(s => s.event.type)).toEqual([
      'UserPromptInjected', 'TextStreamed', 'ResponseGenerated',
    ]);
  });

  it('Human-mode UserPromptInjected after SessionRecovered does NOT absorb (legacy correction)', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'SessionRecovered' }],
      [2, { type: 'UserPromptInjected', text: 'human correction', mode: 'human' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    // Two exchanges: SessionRecovered, then UserPromptInjected as its own boundary.
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].userEvent.type).toBe('SessionRecovered');
    expect(exchanges[1].userEvent.type).toBe('UserPromptInjected');
  });

  // Engine-restart-then-recovered: abort + later same-id terminal must stay
  // in the originating exchange so the rerun's TextStreamed/ResponseGenerated
  // render in the response panel (otherwise they land on a ResponseAborted
  // boundary exchange whose response panel is suppressed).
  it('legacy supersede: ResponseAborted with later same-id terminal stays in originating exchange', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'try this' }],
      [2, { type: 'ResponseAborted', text: 'restart', request_event_id: 'req-1' }],
      [3, { type: 'TextStreamed', text: 'final answer' }],
      [4, { type: 'ResponseGenerated', request_event_id: 'req-1' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual([
      'ResponseAborted', 'TextStreamed', 'ResponseGenerated',
    ]);
  });

  it('non-superseded ResponseAborted (no later same-id terminal) still splits', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'try this' }],
      [2, { type: 'ResponseAborted', text: 'restart', request_event_id: 'req-1' }],
      // Later terminal with DIFFERENT request_event_id — does not supersede.
      [3, { type: 'MessageReceived', text: 'next' }],
      [4, { type: 'ResponseGenerated', request_event_id: 'req-2' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[1].userEvent.type).toBe('ResponseAborted');
    expect(exchanges[2].userEvent.type).toBe('MessageReceived');
  });

  // Mid-flight misattribution: chat agentic loop's request_event_id never
  // re-anchors, so A's late events land after B's MessageReceived in the DB
  // but must still route to A by request_event_id, not chronological position.
  it('routes pre-injection events to A and post-injection events to B (UPI is the split)', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'A', _eventId: 'A' }],
      [2, { type: 'ToolCalled', name: 'web_search', args: {}, request_event_id: 'A' } as StoredEvent],
      [3, { type: 'MessageReceived', text: 'B', _eventId: 'B' }],
      [4, { type: 'ToolResult', name: 'web_search', result: 'ok', request_event_id: 'A' } as StoredEvent],
      // UPI is the moment the loop ingested the queued prompt — every event
      // after this is part of B's answer even though the loop keeps stamping
      // them with A's req_id.
      [5, { type: 'UserPromptInjected', text: 'B', injected_message_id: 'B', request_event_id: 'A' } as StoredEvent],
      [6, { type: 'TextStreamed', text: 'final answer', request_event_id: 'A' } as StoredEvent],
      [7, { type: 'ResponseGenerated', text: 'final answer', request_event_id: 'A' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);

    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect((exchanges[0].userEvent as { text: string }).text).toBe('A');
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual([
      'ToolCalled', 'ToolResult',
    ]);

    expect(exchanges[1].userEvent.type).toBe('MessageReceived');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('B');
    expect(exchanges[1].steps.map(s => s.event.type)).toEqual([
      'UserPromptInjected', 'TextStreamed', 'ResponseGenerated',
    ]);
  });

  it('routes a late ResponseAborted to A (terminating it) and still opens a boundary exchange', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'A', _eventId: 'A' }],
      [2, { type: 'ToolCalled', name: 'web_search', args: {}, request_event_id: 'A' } as StoredEvent],
      [3, { type: 'MessageReceived', text: 'B', _eventId: 'B' }],
      [4, { type: 'ResponseAborted', text: '', request_event_id: 'A' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(3);

    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect((exchanges[0].userEvent as { text: string }).text).toBe('A');
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual([
      'ToolCalled', 'ResponseAborted',
    ]);

    expect(exchanges[1].userEvent.type).toBe('MessageReceived');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('B');
    expect(exchanges[1].steps).toHaveLength(0);

    expect(exchanges[2].userEvent.type).toBe('ResponseAborted');
    expect(exchanges[2].steps).toHaveLength(0);
  });

  it('falls back to the current exchange when request_event_id has no matching anchor', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'A', _eventId: 'A' }],
      [2, { type: 'TextStreamed', text: 'partial', request_event_id: 'orphan' } as StoredEvent],
      [3, { type: 'ResponseGenerated', text: 'done', request_event_id: 'orphan' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual([
      'TextStreamed', 'ResponseGenerated',
    ]);
  });

  // CC sessions reuse one `request_event_id` across mid-flight follow-ups (the
  // session's meta is stamped once at start and never re-anchored). When the
  // user injects a new MessageReceived B mid-flight and then cancels, the
  // resulting ResponseCanceled carries A's req_id but semantically terminates
  // whatever is currently running — which is exchange B. Routing it back to A
  // by req_id leaves B with no terminal, so it shows "Working" forever (or
  // "Done" once a recovery CodingAgentIdled lands).
  it('CC: ResponseCanceled with old session req_id routes to the latest CC exchange (mid-flight cancel)', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'A', channel: 'claude_code', _eventId: 'A' }],
      [2, { type: 'CodingAgentTextStreamed', text: 'thinking', request_event_id: 'A' } as StoredEvent],
      [3, { type: 'CodingAgentToolCalled', name: 'Read', args: {}, request_event_id: 'A' } as StoredEvent],
      [4, { type: 'MessageReceived', text: 'B follow-up', channel: 'claude_code', _eventId: 'B' }],
      [5, { type: 'CodingAgentTextStreamed', text: 'continuing', request_event_id: 'A' } as StoredEvent],
      [6, { type: 'CodingAgentPromptSent', text: 'B follow-up', request_event_id: 'A' } as StoredEvent],
      [7, { type: 'ResponseCanceled', channel: 'claude_code', request_event_id: 'A' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].steps.map(s => s.event.type)).toContain('ResponseCanceled');
  });

  // Chat threads still need request_event_id routing (each chat exchange has
  // its own req_id; late events from A must route back to A).
  it('chat: ResponseCanceled with old req_id still routes to the originating exchange', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'A', channel: 'chat', _eventId: 'A' }],
      [2, { type: 'TextStreamed', text: 'thinking', request_event_id: 'A' } as StoredEvent],
      [3, { type: 'MessageReceived', text: 'B', channel: 'chat', _eventId: 'B' }],
      [4, { type: 'ResponseCanceled', channel: 'chat', request_event_id: 'A' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].steps.map(s => s.event.type)).toContain('ResponseCanceled');
    expect(exchanges[1].steps.map(s => s.event.type)).not.toContain('ResponseCanceled');
  });

  // Legacy engine-spawned CC threads (merge-conflict, hardening) created before
  // MergeConflictDetected/MissingHardeningDetected boundary events existed
  // emit a bare CodingAgentPromptSent as the first content event. Without a
  // boundary the exchange builder dropped every following step and returned
  // zero exchanges, surfacing as the "Messages could not be displayed" empty
  // state. Promote the orphaned prompt to its own boundary so the panel renders.
  it('promotes a leading CodingAgentPromptSent to an exchange-start when no boundary precedes it', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'CodingAgentPromptSent', text: 'Resolve the merge conflict in foo.rs.' }],
      [2, { type: 'SessionStarted', session_id: 's1' }],
      [3, { type: 'CodingAgentToolCalled', name: 'Read', args: {} }],
      [4, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' }],
      [5, { type: 'CodingAgentTextStreamed', text: 'Conflict resolved.' }],
      [6, { type: 'ResponseGenerated' }],
      [7, { type: 'CodingAgentIdled' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].userEvent.type).toBe('CodingAgentPromptSent');
    expect(exchanges[0].userSeq).toBe(1);
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual([
      'SessionStarted',
      'CodingAgentToolCalled',
      'CodingAgentToolResult',
      'CodingAgentTextStreamed',
      'ResponseGenerated',
      'CodingAgentIdled',
    ]);
  });

  // Modern engine-spawned threads emit MergeConflictDetected first; the
  // following CodingAgentPromptSent must stay as a step under that boundary,
  // not split into a second exchange.
  it('does NOT split when CodingAgentPromptSent follows a MergeConflictDetected boundary', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MergeConflictDetected', change_id: 'c1', files: ['foo.rs'] }],
      [2, { type: 'CodingAgentPromptSent', text: 'Resolve the merge conflict.' }],
      [3, { type: 'CodingAgentTextStreamed', text: 'On it.' }],
      [4, { type: 'CodingAgentIdled' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].userEvent.type).toBe('MergeConflictDetected');
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual([
      'CodingAgentPromptSent',
      'CodingAgentTextStreamed',
      'CodingAgentIdled',
    ]);
  });
});

describe('unresumedAbortIndex', () => {
  it('returns the latest aborted exchange when no SessionRecovered follows', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted' }],
      [3, { type: 'MessageReceived', text: 'two' }],
      [4, { type: 'ResponseAborted' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    // 4 exchanges: msg, abort, msg, abort
    expect(unresumedAbortIndex(exchanges)).toBe(3);
  });

  it('returns null when the last abort has been resumed', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted' }],
      [3, { type: 'SessionRecovered' }],
      [4, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(unresumedAbortIndex(exchanges)).toBeNull();
  });

  it('returns null when there are no aborts', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(unresumedAbortIndex(exchanges)).toBeNull();
  });
});

describe('resumeEngineNote', () => {
  it('reads the engine note from a SessionRecovered exchange and counts tool bullets', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'SessionRecovered' }],
      [2, { type: 'UserPromptInjected', mode: 'engine', text:
        '[Engine note — this is a rerun]\n' +
        'Your previous attempt at this turn was interrupted by an engine restart.\n' +
        'The interrupted run performed the following actions before the abort:\n' +
        '- send_notification(Hi) → ok\n' +
        '- read_file(foo.txt) → contents\n' +
        '- run_bash(ls) → README.md',
      }],
    ]);
    const exchanges = groupIntoExchanges(events);
    const note = resumeEngineNote(exchanges[0]);
    expect(note).not.toBeNull();
    expect(note!.toolCount).toBe(3);
    expect(note!.text).toContain('Engine note');
  });

  it('returns null when the SessionRecovered has no engine UserPromptInjected step', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'SessionRecovered' }],
      // CC resume path emits SessionRecovered alone (no engine note).
      [2, { type: 'CodingAgentTextStreamed', text: 'Continuing.' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(resumeEngineNote(exchanges[0])).toBeNull();
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

  // Free-form CC question answers route through process.rs's
  // answer_pending_question path, which emits UserQuestionAnswered (FreeText)
  // but never a MessageReceived. Without explicit cleanup, the optimistic
  // pendingUserMessage from sendMessage() lives until the 30s safety timer,
  // and computeExchanges synthesizes it as a duplicate "You" exchange below
  // the question card's "YOUR ANSWER" panel.
  it('clears matching pendingUserMessage on UserQuestionAnswered with FreeText answer', () => {
    const thread = makeThreadState();
    thread.pendingUserMessages = [
      { text: 'Ask anyway but proceed if reversible', eventId: 'msg-1', created: '2026-05-04T07:45:00Z' },
    ];
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 10, {
      type: 'UserQuestionAnswered',
      tool_use_id: 'tu-1',
      answer: { kind: 'FreeText', text: 'Ask anyway but proceed if reversible' },
    } as ThreadEvent, TS);

    expect(thread.pendingUserMessages).toEqual([]);
  });

  // Selected answers come from the option-button POST path which never adds
  // a pendingUserMessage in the first place — so any pending message in the
  // queue belongs to an unrelated typed-input flow and must NOT be cleared.
  it('does not clear pendingUserMessages on UserQuestionAnswered with Selected answer', () => {
    const thread = makeThreadState();
    thread.pendingUserMessages = [
      { text: 'unrelated typed message', eventId: 'msg-1', created: '2026-05-04T07:45:00Z' },
    ];
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 10, {
      type: 'UserQuestionAnswered',
      tool_use_id: 'tu-1',
      answer: { kind: 'Selected', option_id: 'opt-0' },
    } as ThreadEvent, TS);

    expect(thread.pendingUserMessages).toHaveLength(1);
  });

  // Full integration: typed answer to a CC AskUserQuestion must render only
  // inside the question's divider, not as a separate "You" exchange below.
  it('does not duplicate user answer as a separate exchange after UserQuestionAnswered (FreeText)', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'help me' }, '2026-05-04T07:44:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'SessionStarted', session_id: 's', branch: 'b' } as ThreadEvent, '2026-05-04T07:44:30Z');
    handleEvent(map, 'thread-1', 3, {
      type: 'UserQuestionAsked',
      tool_use_id: 'tu-1',
      cc_session_id: 's',
      question: 'X or Y?',
      options: [],
    } as ThreadEvent, '2026-05-04T07:45:00Z');

    // sendMessage's optimistic update for the user-typed answer
    thread.pendingUserMessages.push({
      text: 'Y',
      eventId: 'msg-optimistic',
      created: '2026-05-04T07:45:01Z',
    });

    // Backend routes the typed text to answer_pending_question
    handleEvent(map, 'thread-1', 4, {
      type: 'UserQuestionAnswered',
      tool_use_id: 'tu-1',
      answer: { kind: 'FreeText', text: 'Y' },
    } as ThreadEvent, '2026-05-04T07:45:02Z');

    const exchanges = computeExchanges(thread);
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'UserQuestionAsked',
    ]);
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

  it('UserPromptInjected with injected_message_id absorbs into matching MessageReceived exchange', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'first', _eventId: 'msg-1' }],
      [2, { type: 'ResponseGenerated', text: 'first response' }],
      [3, { type: 'MessageReceived', text: 'follow-up', _eventId: 'msg-2' }],
      [4, { type: 'UserPromptInjected', text: 'follow-up', injected_message_id: 'msg-2' }],
      [5, { type: 'TextStreamed', text: 'working on it' }],
      [6, { type: 'ResponseGenerated', text: 'done' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect((exchanges[0].userEvent as { text: string }).text).toBe('first');
    expect(exchanges[1].userEvent.type).toBe('MessageReceived');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('follow-up');
    expect(exchanges[1].steps.map(s => s.event.type)).toEqual([
      'UserPromptInjected', 'TextStreamed', 'ResponseGenerated',
    ]);
  });

  it('UserPromptInjected without injected_message_id still starts its own exchange (legacy)', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'start', _eventId: 'msg-1' }],
      [2, { type: 'UserPromptInjected', text: 'inject' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('UserPromptInjected');
  });

  it('UserPromptInjected with injected_message_id and no matching MessageReceived falls back to its own exchange', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'start', _eventId: 'msg-1' }],
      [2, { type: 'UserPromptInjected', text: 'orphan', injected_message_id: 'missing' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('UserPromptInjected');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('orphan');
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

  it('parallel CC reads with same description: result pairs by tool_use_id, not visual order', () => {
    // Two CC `Read SKILL.md` calls run in parallel — same row label, different
    // paths, different tool_use_ids. The result for the first call arrives
    // before the second; the row that gets resolved must be the one whose
    // tool_use_id matches, not whichever pending row came last in the events.
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'audit skills', created: '2026-04-04T10:00:00Z' } as ThreadEvent],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-04-04T10:00:01Z' } as ThreadEvent],
      [3, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: '/skills/grill-me/SKILL.md' }, tool_use_id: 'tu-A', created: '2026-04-04T10:00:02Z' } as ThreadEvent],
      [4, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: '/skills/superpowers/SKILL.md' }, tool_use_id: 'tu-B', created: '2026-04-04T10:00:02Z' } as ThreadEvent],
      // Only the result for the FIRST call has arrived so far.
      [5, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', tool_use_id: 'tu-A', created: '2026-04-04T10:00:03Z' } as ThreadEvent],
    ]);

    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);

    const respSteps = exchangeResponseEvents(exchanges[0]).filter(e => e.type === 'step') as { tool_use_id?: string; success: boolean | null }[];
    expect(respSteps).toHaveLength(2);
    const stepA = respSteps.find(s => s.tool_use_id === 'tu-A');
    const stepB = respSteps.find(s => s.tool_use_id === 'tu-B');
    expect(stepA?.success).toBe(true);  // Row that got its result is done
    expect(stepB?.success).toBeNull();  // Other row keeps spinning until its result arrives

    const steps = exchangeSteps(exchanges[0]) as { tool_use_id?: string; success: boolean | null }[];
    expect(steps).toHaveLength(2);
    expect(steps.find(s => s.tool_use_id === 'tu-A')?.success).toBe(true);
    expect(steps.find(s => s.tool_use_id === 'tu-B')?.success).toBeNull();
  });

  it('legacy CC events without tool_use_id fall back to backward-walk resolution', () => {
    // Stored events from before the tool_use_id field existed render with all
    // pending steps eventually resolved by description-based fallback alone.
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix it', created: '2026-04-04T10:00:00Z' } as ThreadEvent],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-04-04T10:00:01Z' } as ThreadEvent],
      [3, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'a.rs' }, created: '2026-04-04T10:00:02Z' } as ThreadEvent],
      [4, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'b.rs' }, created: '2026-04-04T10:00:02Z' } as ThreadEvent],
      [5, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: '2026-04-04T10:00:03Z' } as ThreadEvent],
      [6, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: '2026-04-04T10:00:03Z' } as ThreadEvent],
      [7, { type: 'CodingAgentTextStreamed', text: 'done', created: '2026-04-04T10:00:04Z' } as ThreadEvent],
    ]);

    const exchanges = groupIntoExchanges(events);
    const respSteps = exchangeResponseEvents(exchanges[0]).filter(e => e.type === 'step') as { success: boolean | null }[];
    expect(respSteps).toHaveLength(2);
    for (const step of respSteps) expect(step.success).toBe(true);
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

  it('exchangeResponseEvents stamps full command on engine ToolCalled steps for hover tooltip', () => {
    // The engine truncates run_bash descriptions to ~60 chars (`Running: cd /Users/...`).
    // The full command is preserved on the step so the UI can show it on mouseover.
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    const fullCmd = 'cd /Users/alex/IdeaProjects/lucidos && git log --oneline -50 | head -20';
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'show recent commits' } as ThreadEvent, '2026-04-30T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'ToolCalled', name: 'run_bash', args: { command: fullCmd }, description: 'Running: cd /Users/alex/IdeaProjects/lucidos && git......' } as ThreadEvent, '2026-04-30T10:00:01Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const respEvents = exchangeResponseEvents(exchanges[0]);
    const stepEvent = respEvents.find(e => e.type === 'step') as Extract<typeof respEvents[number], { type: 'step' }> | undefined;
    expect(stepEvent?.full).toBe(fullCmd);
  });

  it('fullCommandForEngineTool returns the un-elided arg for common engine tools', () => {
    expect(fullCommandForEngineTool('run_bash', { command: 'ls -la /tmp' })).toBe('ls -la /tmp');
    expect(fullCommandForEngineTool('run_python', { code: 'print(1)\nprint(2)' })).toBe('print(1)\nprint(2)');
    expect(fullCommandForEngineTool('read_file', { path: '/data/foo.md' })).toBe('/data/foo.md');
    expect(fullCommandForEngineTool('edit_file', { path: '/src/lib.rs' })).toBe('/src/lib.rs');
    expect(fullCommandForEngineTool('http_request', { method: 'POST', url: 'https://api.example.com/x' })).toBe('https://api.example.com/x');
    expect(fullCommandForEngineTool('web_search', { query: 'rust async runtime comparison' })).toBe('rust async runtime comparison');
    expect(fullCommandForEngineTool('emit_event', { event_type: 'TaskCompleted' })).toBe('TaskCompleted');
    expect(fullCommandForEngineTool('send_email', { subject: 'Re: invoice', to: 'a@b.c' })).toBe('Re: invoice');
    expect(fullCommandForEngineTool('list_repositories', {})).toBeUndefined();
    expect(fullCommandForEngineTool('run_bash', null)).toBeUndefined();
    expect(fullCommandForEngineTool('run_bash', { command: 123 })).toBeUndefined();
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

    // Exchange 1's ToolResult ended up in exchange 2. Cancel took the thread
    // idle, which is what resolves the orphaned spinner.
    const ex1Steps = exchangeSteps(exchanges[0], /* isLast */ false, /* threadIdle */ true);
    expect(ex1Steps.filter(s => s.success === null)).toHaveLength(0);

    const ex1Events = exchangeResponseEvents(exchanges[0], 0, /* isLast */ false, /* threadIdle */ true);
    const pendingEvents = ex1Events.filter(e => e.type === 'step' && (e as { success: boolean | null }).success === null);
    expect(pendingEvents).toHaveLength(0);
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
      // CC process dies — safety-net ResponseAborted, also opens its own abort exchange.
      { seq: 7, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
    ]);
    // ResponseAborted dual-purpose: terminates exchange 2 AND opens a new
    // abort boundary exchange (the AbortPanel + Continue button surface).
    expect(exchanges).toHaveLength(3);
    expect(statuses[0]).toBe('done');      // Exchange 1 was already done
    expect(statuses[1]).toBe('aborted');   // Exchange 2 aborted by crash
    expect(exchanges[2].userEvent.type).toBe('ResponseAborted'); // boundary
  });

  it('CC crash on follow-up: ResponseAborted opens its own boundary exchange', () => {
    const { exchanges } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
    ]);
    // 1: idle, 2: aborted follow-up, 3: ResponseAborted boundary
    expect(exchanges).toHaveLength(3);
    expect(exchanges[2].userEvent.type).toBe('ResponseAborted');
  });

  it('engine restart on follow-up: SessionEnded(shutdown) terminates the exchange aborted', () => {
    const { exchanges, statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'SessionEnded', reason: 'shutdown' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
    ]);
    expect(statuses[1]).toBe('aborted');
    expect(exchanges).toHaveLength(2);
  });

  it('lost follow-up during CC exit: ResponseAborted for drained messages = aborted', () => {
    const { statuses, exchanges } = buildCCThread([
      // Exchange 1: normal
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Exchange 2: follow-up sent, but CC was exiting — message lost
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up that got lost', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      // Backend drains lost follow-ups → single ResponseAborted (also opens
      // its own abort boundary exchange).
      { seq: 5, event: { type: 'ResponseAborted', text: '1 follow-up message(s) lost during session exit' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
    ]);
    expect(exchanges).toHaveLength(3);
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
      // Exchange 1: aborted (ResponseAborted appears as a step AND opens an
      // abort boundary exchange between this and the user's retry)
      { seq: 1, event: { type: 'MessageReceived', text: 'first', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // New session — user retries
      { seq: 4, event: { type: 'MessageReceived', text: 'try again', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'SessionStarted', session_id: 's2' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 6, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
    ]);
    expect(exchanges).toHaveLength(3);
    expect(statuses[0]).toBe('aborted');                    // first
    expect(exchanges[1].userEvent.type).toBe('ResponseAborted'); // boundary
    expect(statuses[2]).toBe('done');                       // try again
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

  // Reproduces the user-reported bug: mid-flight follow-up during an active CC
  // session, then user clicks Cancel. The engine's CC-session meta carries the
  // ORIGINAL message's request_event_id for the entire session lifetime, so the
  // emitted ResponseCanceled is anchored to message A's id even though it
  // semantically terminates whatever was running last (B). Engine restart
  // afterward emits a recovery CodingAgentIdled with no req_id. Without the
  // CC channel exemption, ResponseCanceled routes back to A and B shows
  // "Working" (then "Done" once the recovery idle lands) instead of "Canceled".
  it('mid-flight cancel during active CC: follow-up exchange shows canceled (not done) after engine_restart_interrupt idle', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'fix', channel: 'claude_code', _eventId: 'A', created: '2026-04-12T10:00:00Z' }],
      [2, { type: 'SessionStarted', session_id: 's1', request_event_id: 'A', created: '2026-04-12T10:00:01Z' } as StoredEvent],
      [3, { type: 'CodingAgentToolCalled', name: 'Read', args: {}, request_event_id: 'A', created: '2026-04-12T10:00:02Z' } as StoredEvent],
      // Mid-flight follow-up — engine injects via msg_tx, session meta unchanged
      [4, { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code', _eventId: 'B', created: '2026-04-12T10:00:03Z' }],
      [5, { type: 'CodingAgentTextStreamed', text: 'continuing', request_event_id: 'A', created: '2026-04-12T10:00:04Z' } as StoredEvent],
      [6, { type: 'CodingAgentPromptSent', text: 'follow-up', request_event_id: 'A', created: '2026-04-12T10:00:05Z' } as StoredEvent],
      // User clicks cancel — emits ResponseCanceled with the session's req_id (A)
      [7, { type: 'ResponseCanceled', channel: 'claude_code', request_event_id: 'A', created: '2026-04-12T10:00:06Z' } as StoredEvent],
      // Engine restarts later; recovery emits a synthetic idle with no req_id
      [8, { type: 'CodingAgentIdled', reason: 'engine_restart_interrupt', created: '2026-04-12T10:30:00Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    const status = exchangeStatus(exchanges[1], '', true, false, true);
    expect(status).toBe('canceled');
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
      // CC crashes during hardening — also opens a new abort boundary exchange.
      { seq: 12, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:01:07Z' },
    ]);
    expect(exchanges).toHaveLength(3);
    expect(statuses[0]).toBe('done');
    expect(statuses[1]).toBe('done'); // User work was done — harden crash is system-level
    expect(exchanges[2].userEvent.type).toBe('ResponseAborted');
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

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'first turn' } as ThreadEvent, '2026-04-24T10:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-24T10:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, '2026-04-24T10:00:02Z');
    handleEventWithAgg(map, 't1', 4, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, '2026-04-24T10:00:03Z');

    // Idled with no changes → status 'idle' (turn done) but thread is still
    // alive: not 'failed', still in the map, and ready to resume.
    expect(thread.meta.status).toBe('idle');
    expect(map.has('t1')).toBe(true);

    // Follow-up turn brings it back to 'running' — proves the thread wasn't
    // closed by the previous CodingAgentIdled.
    handleEventWithAgg(map, 't1', 5, { type: 'MessageReceived', text: 'second turn' } as ThreadEvent, '2026-04-24T10:00:10Z');
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

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'do work' } as ThreadEvent, '2026-04-24T10:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-24T10:00:01Z');
    // Three commits in the same turn — emitted per-commit by the post-commit hook.
    handleEventWithAgg(map, 't1', 3, { type: 'ChangeProposed', change_id: 'c1', commit_sha: 'aaa111', description: 'first' } as ThreadEvent, '2026-04-24T10:00:02Z');
    handleEventWithAgg(map, 't1', 4, { type: 'ChangeProposed', change_id: 'c1', commit_sha: 'bbb222', description: 'second' } as ThreadEvent, '2026-04-24T10:00:03Z');
    handleEventWithAgg(map, 't1', 5, { type: 'ChangeProposed', change_id: 'c1', commit_sha: 'ccc333', description: 'third' } as ThreadEvent, '2026-04-24T10:00:04Z');

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

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'long task' } as ThreadEvent, '2026-04-24T10:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-24T10:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} } as ThreadEvent, '2026-04-24T10:00:02Z');
    handleEventWithAgg(map, 't1', 4, { type: 'ResponseCanceled' } as ThreadEvent, '2026-04-24T10:00:03Z');

    // ResponseCanceled drops to 'idle' (no changes) but thread is alive,
    // not 'failed', and resumable.
    expect(thread.meta.status).toBe('idle');
    expect(map.has('t1')).toBe(true);

    // Follow-up brings it back to 'running'.
    handleEventWithAgg(map, 't1', 5, { type: 'MessageReceived', text: 'try again' } as ThreadEvent, '2026-04-24T10:00:10Z');
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

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'do work' } as ThreadEvent, '2026-04-24T10:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-24T10:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, '2026-04-24T10:00:02Z');
    handleEventWithAgg(map, 't1', 4, { type: 'SessionEnded', reason: 'shutdown' } as ThreadEvent, '2026-04-24T10:00:03Z');

    // Mid-work shutdown — exchange shows 'aborted', distinguishing it from
    // the active-after-Idled / -Canceled / -ChangeProposed cases above.
    const exchanges = groupIntoExchanges(thread.events);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true, false, true)).toBe('aborted');
  });
});

