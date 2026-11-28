import { describe, it, expect } from 'vitest';
import { TS, makeThreadState } from './thread-events-helpers';
import { applySummaryVersion, abortPromisesAutoResume, abortTookEngineDown, computeExchanges, exchangeKey, exchangeStatus, groupIntoExchanges, handleEvent, isSwitchTeardownAbort, responseAbortedSummary, resumeEngineNote, continuableAbortIndex, type AbortCause, type Exchange, type MessageOrigin, type StoredEvent, type ThreadAggregate, type ThreadEvent, type ThreadState, type TransientEvent } from '../thread-events';

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
      summaryVersion: 0,
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0, archivableSubThreadCount: 0, liveEventWaitCount: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
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
    applySummaryVersion(thread.meta, thread.meta.summaryVersion, 'running');
    const map = new Map([['thread-1', thread]]);
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

  it('leaves side questions out of the turn they were asked during', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'first' }],
      [2, { type: 'SideQuestionAsked', side_question_id: 'a', question: 'what is X?' }],
      [3, { type: 'TextStreamed', text: 'reply' }],
      [4, { type: 'SideQuestionAnswered', side_question_id: 'a', answer: 'X' }],
      [5, { type: 'SideQuestionDismissed', side_question_id: 'a' }],
      [6, { type: 'ResponseGenerated' }],
    ] as [number, ThreadEvent][]);
    const [turn] = groupIntoExchanges(events);
    expect(turn.steps.map((s) => s.seq)).toEqual([3, 6]);
  });

  it('files a late change summary under the turn that proposed the change', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'build it' }],
      [2, { type: 'ChangeProposed', change_id: 'c1', description: 'b\na' }],
      [3, { type: 'ResponseGenerated' }],
      [4, { type: 'ChangeApplied', change_id: 'c1' }],
      [5, { type: 'ChangeSummarized', change_id: 'c1', summary: 'Builds it', description: 'b\na' }],
    ] as [number, ThreadEvent][]);
    const exchanges = groupIntoExchanges(events);
    const card = exchanges.find((ex) => ex.userEvent.type === 'ChangeApplied')!;
    expect(card.steps).toEqual([]);
    expect(exchanges[0].steps.map((s) => s.seq)).toEqual([2, 3, 5]);
  });

  it('drops a change summary whose proposal is outside the loaded window', () => {
    const events = new Map<number, ThreadEvent>([
      [4, { type: 'ChangeApplied', change_id: 'c1' }],
      [5, { type: 'ChangeSummarized', change_id: 'c1', summary: 'Builds it', description: 'b\na' }],
    ] as [number, ThreadEvent][]);
    const [card] = groupIntoExchanges(events);
    expect(card.steps).toEqual([]);
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

  it('does not fold WorktreeCleaned into any exchange, leaving the child card "done"', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'spawn a child' }],
      [2, { type: 'CodingAgentTextStreamed', text: 'Spawned successfully' } as ThreadEvent],
      [3, { type: 'CodingAgentIdled', has_changes: false }],
      [4, { type: 'ChildThreadCompleted', child_thread_id: 'c1', status: 'success', summary: 'done' } as ThreadEvent],
      [5, { type: 'WorktreeCleaned', tier: 0, freed_bytes: 15853 }],
    ]);
    const exchanges = groupIntoExchanges(events);
    const allSteps = exchanges.flatMap(e => e.steps.map(s => s.event.type));
    expect(allSteps).not.toContain('WorktreeCleaned');
    const card = exchanges[exchanges.length - 1];
    expect(card.userEvent.type).toBe('ChildThreadCompleted');
    expect(card.steps).toHaveLength(0);
    expect(
      exchangeStatus(card, '',  true,  false,  true,  true),
    ).toBe('done');
  });

  it('splits at ResponseAborted: terminates prior exchange AND opens an empty boundary exchange', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix the bug' }],
      [2, { type: 'TextStreamed', text: 'Working...' }],
      [3, { type: 'ResponseAborted', text: 'Working...' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual(['TextStreamed', 'ResponseAborted']);
    expect(exchanges[1].userEvent.type).toBe('ResponseAborted');
    expect(exchanges[1].steps).toHaveLength(0);
  });

  it('splits at ContinuationStarted: opens a resume exchange that absorbs the engine note', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix the bug' }],
      [2, { type: 'ResponseAborted', text: '' }],
      [3, { type: 'ContinuationStarted' }],
      [4, { type: 'PromptInjected', text: '[Engine note]', mode: 'engine' }],
      [5, { type: 'TextStreamed', text: 'On it.' }],
      [6, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[1].userEvent.type).toBe('ResponseAborted');
    expect(exchanges[2].userEvent.type).toBe('ContinuationStarted');
    expect(exchanges[2].steps.map(s => s.event.type)).toEqual([
      'PromptInjected', 'TextStreamed', 'ResponseGenerated',
    ]);
  });

  it('Human-mode PromptInjected after ContinuationStarted does NOT absorb (legacy correction)', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'ContinuationStarted' }],
      [2, { type: 'PromptInjected', text: 'human correction', mode: 'human' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].userEvent.type).toBe('ContinuationStarted');
    expect(exchanges[1].userEvent.type).toBe('PromptInjected');
  });

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
      [3, { type: 'MessageReceived', text: 'next' }],
      [4, { type: 'ResponseGenerated', request_event_id: 'req-2' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[1].userEvent.type).toBe('ResponseAborted');
    expect(exchanges[2].userEvent.type).toBe('MessageReceived');
  });

  it('routes pre-injection events to A and post-injection events to B (PromptInjected is the split)', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'A', _eventId: 'A' }],
      [2, { type: 'ToolCalled', name: 'web_search', args: {}, request_event_id: 'A' } as StoredEvent],
      [3, { type: 'MessageReceived', text: 'B', _eventId: 'B' }],
      [4, { type: 'ToolResult', name: 'web_search', result: 'ok', request_event_id: 'A' } as StoredEvent],
      [5, { type: 'PromptInjected', text: 'B', injected_message_id: 'B', request_event_id: 'A' } as StoredEvent],
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
      'PromptInjected', 'TextStreamed', 'ResponseGenerated',
    ]);
  });

  it('routes ContextCaptured/MemoryRecalled (+ legacy predecessors) by request_event_id, not current pointer', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'A', _eventId: 'A' }],
      [2, { type: 'TextStreamed', text: 'a-reply', request_event_id: 'A' } as StoredEvent],
      [3, { type: 'ResponseGenerated', text: 'a-reply', request_event_id: 'A' } as StoredEvent],
      [4, { type: 'MessageReceived', text: 'B', _eventId: 'B' }],
      [5, { type: 'MemoryRecalled', queries: [], results: 0, request_event_id: 'A' } as unknown as StoredEvent],
      [6, { type: 'ContextCaptured', producer: 'chat', model: 'm', context_window: 0, sections: [], tools: [], estimated_total_tokens: 0, request_event_id: 'A' } as unknown as StoredEvent],
      [7, { type: 'ContextAssembled', sections: [], tools: [], model: 'm', total_chars: 0, request_event_id: 'A' } as unknown as StoredEvent],
      [8, { type: 'ContextTokensMeasured', tokens: 100, message_count: 1, request_event_id: 'A' } as unknown as StoredEvent],
      [9, { type: 'MemorySearched', queries: [], results: 0, request_event_id: 'A' } as unknown as StoredEvent],
      [10, { type: 'TextStreamed', text: 'late', request_event_id: 'A' } as StoredEvent],
      [11, { type: 'ResponseGenerated', text: 'late', request_event_id: 'A' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);

    expect((exchanges[0].userEvent as { text: string }).text).toBe('A');
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual([
      'TextStreamed',
      'ResponseGenerated',
      'MemoryRecalled',
      'ContextCaptured',
      'ContextAssembled',
      'ContextTokensMeasured',
      'MemorySearched',
      'TextStreamed',
      'ResponseGenerated',
    ]);

    expect((exchanges[1].userEvent as { text: string }).text).toBe('B');
    expect(exchanges[1].steps).toHaveLength(0);
  });

  it('does not flash "aborted" on a follow-up when the prior response\'s ContextCaptured arrives late', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'Min side > Innboks > Kontakt oss?', _eventId: 'A' }],
      [2, { type: 'MemorySearched', queries: [], results: [], request_event_id: 'A' } as unknown as StoredEvent],
      [3, { type: 'ThoughtStreamed', text: '', request_event_id: 'A' } as unknown as StoredEvent],
      [4, { type: 'MessageReceived', text: 'hmm?', _eventId: 'B' }],
      [5, { type: 'ContextCaptured', producer: 'chat', model: 'm', context_window: 0, sections: [], tools: [], estimated_total_tokens: 0, request_event_id: 'A' } as unknown as StoredEvent],
      [6, { type: 'TextStreamed', text: 'a-reply', request_event_id: 'A' } as StoredEvent],
      [7, { type: 'ResponseGenerated', text: 'a-reply', request_event_id: 'A' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);

    expect((exchanges[0].userEvent as { text: string }).text).toBe('Min side > Innboks > Kontakt oss?');
    expect(exchanges[0].steps.map(s => s.event.type)).toContain('ContextCaptured');

    const followUp = exchanges[1];
    expect((followUp.userEvent as { text: string }).text).toBe('hmm?');
    expect(followUp.steps).toHaveLength(0);

    expect(
      exchangeStatus(followUp, '',  true,  false,  false,  true),
    ).toBe('done');
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
    expect(exchanges).toHaveLength(3);
    expect(exchanges[1].steps.map(s => s.event.type)).toContain('ResponseCanceled');
    expect(exchanges[2].userEvent.type).toBe('ResponseCanceled');
  });

  it('Codex: superseded_by_followup ResponseCanceled does NOT open a cancel boundary exchange', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'A', channel: 'claude_code', _eventId: 'A' }],
      [2, { type: 'CodingAgentTextStreamed', text: 'working', request_event_id: 'A' } as StoredEvent],
      [3, { type: 'ResponseCanceled', channel: 'claude_code', request_event_id: 'A', cause: 'superseded_by_followup' } as StoredEvent],
      [4, { type: 'CodingAgentIdled', request_event_id: 'A' } as StoredEvent],
      [5, { type: 'MessageReceived', text: 'B follow-up', channel: 'claude_code', _eventId: 'B' }],
      [6, { type: 'CodingAgentTextStreamed', text: 'on the follow-up', request_event_id: 'A' } as StoredEvent],
      [7, { type: 'ResponseGenerated', request_event_id: 'A' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].steps.map(s => s.event.type)).toContain('ResponseCanceled');
    expect(exchanges.some(e => e.userEvent.type === 'ResponseCanceled')).toBe(false);
  });

  it('chat: ResponseCanceled with old req_id still routes to the originating exchange', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'A', channel: 'chat', _eventId: 'A' }],
      [2, { type: 'TextStreamed', text: 'thinking', request_event_id: 'A' } as StoredEvent],
      [3, { type: 'MessageReceived', text: 'B', channel: 'chat', _eventId: 'B' }],
      [4, { type: 'ResponseCanceled', channel: 'chat', request_event_id: 'A' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[0].steps.map(s => s.event.type)).toContain('ResponseCanceled');
    expect(exchanges[1].steps.map(s => s.event.type)).not.toContain('ResponseCanceled');
    expect(exchanges[2].userEvent.type).toBe('ResponseCanceled');
  });

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

describe('continuableAbortIndex', () => {
  it('returns the latest aborted exchange when no ContinuationStarted follows', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted' }],
      [3, { type: 'MessageReceived', text: 'two' }],
      [4, { type: 'ResponseAborted' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBe(3);
  });

  it('returns null when the last abort has been resumed', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted' }],
      [3, { type: 'ContinuationStarted' }],
      [4, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBeNull();
  });

  it('returns null when there are no aborts', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBeNull();
  });

  it('returns null when the latest abort is stale-settle', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted', cause: 'stale_settle' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBeNull();
  });

  it('returns null when a stale-settle abort sits above an older real abort', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted', cause: 'engine_shutdown' }],
      [3, { type: 'MessageReceived', text: 'two' }],
      [4, { type: 'ResponseAborted', cause: 'stale_settle' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBeNull();
  });

  it('returns null on a switch-teardown abort: the engine is auto-resuming it', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, {
        type: 'ResponseAborted',
        cause: 'engine_shutdown',
        actor: { kind: 'device', device_id: 'd1' },
      }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBeNull();
  });

  it('offers Continue on an engine_shutdown abort with no device actor', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted', cause: 'engine_shutdown', actor: { kind: 'system' } }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBe(1);
  });

  it('offers Continue on the boot floor abort that withdraws a resume promise', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, {
        type: 'ResponseAborted',
        cause: 'engine_shutdown',
        actor: { kind: 'device', device_id: 'd1' },
      }],
      [3, { type: 'ResponseAborted', cause: 'recovery_after_restart', actor: { kind: 'system' } }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBe(2);
  });

  it('returns null when the latest abort has since produced a terminal', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted', cause: 'safety_net' }],
      [3, { type: 'TextStreamed', text: 'carrying on' }],
      [4, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBeNull();
  });

  it('skips a resolved abort and offers the older unresolved one', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted', cause: 'safety_net' }],
      [3, { type: 'MessageReceived', text: 'two' }],
      [4, { type: 'ResponseAborted', cause: 'safety_net' }],
      [5, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBe(1);
  });

  it('still offers Continue on an abort with steps but no terminal', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted', cause: 'safety_net' }],
      [3, { type: 'TextStreamed', text: 'partial, then the engine died again' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBe(1);
  });

  it('offers Continue on the recovery pair, whose idle IS the interruption marker', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted', cause: 'recovery_after_restart', actor: { kind: 'system' } }],
      [3, { type: 'CodingAgentIdled', reason: 'engine_restart_interrupt' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBe(1);
  });

  it('returns null when a real coding-agent turn under the abort went idle', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted', cause: 'recovery_after_restart', actor: { kind: 'system' } }],
      [3, { type: 'CodingAgentTextStreamed', text: 'carrying on' }],
      [4, { type: 'CodingAgentIdled' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBeNull();
  });

  it('offers Continue on a crash-aborted coding-agent follow-up', () => {
    const cc = { channel: 'claude_code', request_event_id: 'req-1' };
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one', channel: 'claude_code' }],
      [2, { type: 'ResponseGenerated', text: 'done', ...cc } as ThreadEvent],
      [3, { type: 'MessageReceived', text: 'two', channel: 'claude_code' }],
      [4, { type: 'ResponseAborted', cause: 'recovery_after_restart', actor: { kind: 'system' }, ...cc } as ThreadEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(ex => ex.userEvent.type)).toEqual([
      'MessageReceived',
      'MessageReceived',
      'ResponseAborted',
    ]);
    expect(continuableAbortIndex(exchanges)).toBe(2);
  });
});

describe('abortPromisesAutoResume', () => {
  const device = { kind: 'device', device_id: 'd1' } as const;

  it('matches only engine_shutdown AND a device actor', () => {
    expect(abortPromisesAutoResume(
      { type: 'ResponseAborted', cause: 'engine_shutdown', actor: device },
    )).toBe(true);
    expect(abortPromisesAutoResume(
      { type: 'ResponseAborted', cause: 'engine_shutdown', actor: { kind: 'system' } },
    )).toBe(false);
    expect(abortPromisesAutoResume(
      { type: 'ResponseAborted', cause: 'stale_settle', actor: device },
    )).toBe(false);
    expect(abortPromisesAutoResume(
      { type: 'ResponseAborted', cause: 'recovery_after_restart', actor: device },
    )).toBe(false);
    expect(abortPromisesAutoResume({ type: 'ResponseAborted' })).toBe(false);
    expect(abortPromisesAutoResume({ type: 'ResponseGenerated' })).toBe(false);
  });

  it('agrees with the transcript label on every shape', () => {
    const cases: { actor?: MessageOrigin; cause?: AbortCause }[] = [
      { cause: 'engine_shutdown', actor: device },
      { cause: 'engine_shutdown', actor: { kind: 'system' } },
      { cause: 'engine_shutdown' },
      { cause: 'recovery_after_restart', actor: device },
      { cause: 'recovery_after_restart', actor: { kind: 'system' } },
      { cause: 'process_killed', actor: device },
      { cause: 'safety_net' },
      {},
    ];
    for (const { actor, cause } of cases) {
      const promised = abortPromisesAutoResume({ type: 'ResponseAborted', actor, cause });
      expect(isSwitchTeardownAbort(actor, cause)).toBe(promised);
      expect(responseAbortedSummary(actor, cause)).toBe(
        promised ? 'Paused by restart' : 'Response interrupted',
      );
    }
  });

  it('never reads a device-attributed stale_settle as a switch', () => {
    expect(isSwitchTeardownAbort(device, 'stale_settle')).toBe(false);
    expect(responseAbortedSummary(device, 'stale_settle')).toBe('Stuck reply cleared');
  });

  it('separates "the engine is gone" from "the engine promised to resume"', () => {
    const cases: { cause?: AbortCause; actor?: MessageOrigin; down: boolean; promised: boolean }[] = [
      { cause: 'engine_shutdown', actor: device, down: true, promised: true },
      { cause: 'engine_shutdown', actor: { kind: 'system' }, down: true, promised: false },
      { cause: 'engine_shutdown', down: true, promised: false },
      { cause: 'safety_net', down: false, promised: false },
      { cause: 'recovery_after_restart', actor: device, down: false, promised: false },
      { cause: 'stale_settle', actor: device, down: false, promised: false },
      { cause: 'process_killed', down: false, promised: false },
      { down: false, promised: false },
    ];
    for (const { cause, actor, down, promised } of cases) {
      const ev = { type: 'ResponseAborted', cause, actor } as ThreadEvent;
      expect(abortTookEngineDown(ev), `down for ${cause} / ${actor?.kind}`).toBe(down);
      expect(abortPromisesAutoResume(ev), `promised for ${cause} / ${actor?.kind}`).toBe(promised);
    }
    expect(abortTookEngineDown({ type: 'ResponseGenerated' })).toBe(false);
    expect(abortTookEngineDown({ type: 'ResponseCanceled', cause: 'user_stop' })).toBe(false);
  });
});

describe('resumeEngineNote', () => {
  it('reads the engine note from a ContinuationStarted exchange and counts tool bullets', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'ContinuationStarted' }],
      [2, { type: 'PromptInjected', mode: 'engine', text:
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

  it('returns null when the ContinuationStarted has no engine PromptInjected step', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'ContinuationStarted' }],
      [2, { type: 'CodingAgentTextStreamed', text: 'Continuing.' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(resumeEngineNote(exchanges[0])).toBeNull();
  });
});

describe('handleEvent', () => {
  it('inserts persisted events', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);
    const event: ThreadEvent = { type: 'MessageReceived', text: 'hi' };

    const result = handleEvent(threadMap, 'thread-1', 1, event, TS);

    expect(result.applied).toBe(true);
    expect(thread.events.get(1)).toEqual(expect.objectContaining(event));
  });

  it('deduplicates by sequence number', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);
    const event1: ThreadEvent = { type: 'MessageReceived', text: 'hi' };
    const event2: ThreadEvent = { type: 'MessageReceived', text: 'different' };

    handleEvent(threadMap, 'thread-1', 1, event1, TS);
    const result = handleEvent(threadMap, 'thread-1', 1, event2, TS);

    expect(result.applied).toBe(false);
    expect(thread.events.get(1)).toEqual(expect.objectContaining(event1));
  });

  it('takes the latest cumulative transient text into the streaming buffer', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);
    const transient: TransientEvent = { type: 'CumulativeTextUpdated', text: 'hel' };

    handleEvent(threadMap, 'thread-1', null, transient);
    expect(thread.streamingBuffer).toBe('hel');

    handleEvent(threadMap, 'thread-1', null, { type: 'CumulativeTextUpdated', text: 'hello' });
    expect(thread.streamingBuffer).toBe('hello');
  });

  it('clears streaming buffer on persisted event', () => {
    const thread = makeThreadState();
    thread.streamingBuffer = 'partial text';
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'TextStreamed', text: 'full text' }, TS);
    expect(thread.streamingBuffer).toBe('');
  });

  it('keeps the streaming buffer when a side question lands mid-reply', () => {
    const thread = makeThreadState();
    thread.streamingBuffer = 'partial text';
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'SideQuestionAsked', side_question_id: 'a', question: 'q' } as ThreadEvent, TS);
    expect(thread.streamingBuffer).toBe('partial text');
  });

  it('ignores unknown threads', () => {
    const threadMap = new Map<string, ThreadState>();
    const result = handleEvent(threadMap, 'nonexistent', 1, { type: 'MessageReceived', text: 'hi' }, TS);
    expect(result.applied).toBe(false);
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
    expect(thread.meta.updatedAt).toBe(t1);
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
    expect(thread.meta.updatedAt).toBe(t1);
  });

  it('does NOT update updatedAt on ChangeProposed (status-only, not activity)', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const threadMap = new Map([['thread-1', thread]]);

    const t1 = '2026-03-15T12:00:00Z';
    handleEvent(threadMap, 'thread-1', 1, { type: 'CodingAgentIdled' }, t1);

    const t2 = '2026-03-15T12:01:00Z';
    handleEvent(threadMap, 'thread-1', 2, { type: 'ChangeProposed', change_id: 'c1' }, t2);
    expect(thread.meta.updatedAt).toBe(t1);
  });

  it('does NOT update updatedAt on ResponseCanceled (status-only)', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);

    const t1 = '2026-03-15T12:00:00Z';
    handleEvent(threadMap, 'thread-1', 1, { type: 'MessageReceived', text: 'hi' }, t1);

    const t2 = '2026-03-15T12:01:00Z';
    handleEvent(threadMap, 'thread-1', 2, { type: 'ResponseCanceled' }, t2);
    expect(thread.meta.updatedAt).toBe(t1);
  });

  it('does NOT update updatedAt on metadata events (ThreadTitleGenerated)', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);

    const t1 = '2026-03-15T12:00:00Z';
    handleEvent(threadMap, 'thread-1', 1, { type: 'MessageReceived', text: 'hi' }, t1);

    const t2 = '2026-03-15T12:10:00Z';
    handleEvent(threadMap, 'thread-1', 2, { type: 'ThreadTitleGenerated', title: 'Test' }, t2);
    expect(thread.meta.updatedAt).toBe(t1);
  });

  it('clears matching pendingUserMessage on MessageReceived with matching event_id', () => {
    const thread = makeThreadState();
    thread.pendingUserMessages = [{ text: 'optimistic message', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }];
    const threadMap = new Map([['thread-1', thread]]);

    expect(thread.pendingUserMessages).toHaveLength(1);
    const result = handleEvent(threadMap, 'thread-1', 1, { type: 'MessageReceived', text: 'optimistic message' }, TS, 'msg-1');
    expect(thread.pendingUserMessages).toEqual([]);
    expect(result.clearedPendingUserMessage).toBe(true);
  });

  it('does not clear pendingUserMessages on transient events', () => {
    const thread = makeThreadState();
    thread.pendingUserMessages = [{ text: 'optimistic message', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }];
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', null, { type: 'CumulativeTextUpdated', text: 'chunk' });
    expect(thread.pendingUserMessages).toHaveLength(1);
  });

  it('does not clear pendingUserMessages on non-MessageReceived persisted events', () => {
    const thread = makeThreadState();
    thread.pendingUserMessages = [{ text: 'my question', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }];
    const threadMap = new Map([['thread-1', thread]]);

    const result = handleEvent(threadMap, 'thread-1', 100, { type: 'ToolCalled', name: 'search', args: {} }, TS);

    expect(thread.pendingUserMessages).toHaveLength(1);
    expect(result.clearedPendingUserMessage).toBe(false);

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

  it('clears matching pendingUserMessage on QueuedMessageRemoved', () => {
    const thread = makeThreadState();
    thread.pendingUserMessages = [
      { text: 'first', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' },
      { text: 'second', eventId: 'msg-2', created: '2026-01-01T00:00:00Z' },
    ];
    const threadMap = new Map([['thread-1', thread]]);

    const result = handleEvent(threadMap, 'thread-1', 3, {
      type: 'QueuedMessageRemoved',
      removed_message_id: 'msg-2',
    } as ThreadEvent, TS, 'remove-1');

    expect(thread.pendingUserMessages).toEqual([
      { text: 'first', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' },
    ]);
    expect(result.clearedPendingUserMessage).toBe(true);
  });

  it('hides a removed queued MessageReceived while it remains un-injected', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'MessageReceived', text: 'active' }, TS, 'msg-1');
    handleEvent(threadMap, 'thread-1', 2, { type: 'TextStreamed', text: 'working' }, TS, 'stream-1');
    handleEvent(threadMap, 'thread-1', 3, { type: 'MessageReceived', text: 'queued' }, TS, 'msg-2');
    handleEvent(threadMap, 'thread-1', 4, {
      type: 'QueuedMessageRemoved',
      removed_message_id: 'msg-2',
    } as ThreadEvent, TS, 'remove-1');

    const exchanges = computeExchanges(thread);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect((exchanges[0].userEvent as Extract<ThreadEvent, { type: 'MessageReceived' }>).text).toBe('active');
  });

  it('does not hide a removed message once PromptInjected already attached', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);

    handleEvent(threadMap, 'thread-1', 1, { type: 'MessageReceived', text: 'active' }, TS, 'msg-1');
    handleEvent(threadMap, 'thread-1', 2, { type: 'TextStreamed', text: 'working' }, TS, 'stream-1');
    handleEvent(threadMap, 'thread-1', 3, { type: 'MessageReceived', text: 'queued' }, TS, 'msg-2');
    handleEvent(threadMap, 'thread-1', 4, {
      type: 'QueuedMessageRemoved',
      removed_message_id: 'msg-2',
    } as ThreadEvent, TS, 'remove-1');
    handleEvent(threadMap, 'thread-1', 5, {
      type: 'PromptInjected',
      text: 'queued',
      mode: 'human',
      injected_message_id: 'msg-2',
    } as ThreadEvent, TS, 'inject-1');

    const exchanges = computeExchanges(thread);
    expect(exchanges).toHaveLength(2);
    expect((exchanges[1].userEvent as Extract<ThreadEvent, { type: 'MessageReceived' }>).text).toBe('queued');
    expect(exchanges[1].steps.map(step => step.event.type)).toEqual(['PromptInjected']);
  });

  it('clears matching pendingUserMessage on PromptInjected with matching event_id', () => {
    const thread = makeThreadState();
    thread.pendingUserMessages = [{ text: 'fix the bug', eventId: 'inject-1', created: '2026-01-01T00:00:00Z' }];
    const threadMap = new Map([['thread-1', thread]]);

    const result = handleEvent(threadMap, 'thread-1', 10, { type: 'PromptInjected', text: 'fix the bug' }, TS, 'inject-1');
    expect(thread.pendingUserMessages).toEqual([]);
    expect(result.clearedPendingUserMessage).toBe(true);
  });

  it('clears matching pendingUserMessage on UserQuestionAnswered with FreeText answer', () => {
    const thread = makeThreadState();
    thread.pendingUserMessages = [
      { text: 'Ask anyway but proceed if reversible', eventId: 'msg-1', created: '2026-05-04T07:45:00Z' },
    ];
    const threadMap = new Map([['thread-1', thread]]);

    const result = handleEvent(threadMap, 'thread-1', 10, {
      type: 'UserQuestionAnswered',
      tool_use_id: 'tu-1',
      answer: { kind: 'FreeText', text: 'Ask anyway but proceed if reversible' },
    } as ThreadEvent, TS);

    expect(thread.pendingUserMessages).toEqual([]);
    expect(result.clearedPendingUserMessage).toBe(true);
  });

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

    thread.pendingUserMessages.push({
      text: 'Y',
      eventId: 'msg-optimistic',
      created: '2026-05-04T07:45:01Z',
    });

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


describe('exchangeKey', () => {
  it('keeps the same key when an optimistic pending message becomes a persisted MessageReceived', () => {
    const thread = makeThreadState();
    const EVENT_ID = 'client-uuid-1';
    thread.pendingUserMessages.push({ text: 'logo must match height', eventId: EVENT_ID, created: TS });
    const map = new Map([['thread-1', thread]]);

    const optimistic = computeExchanges(thread);
    expect(optimistic).toHaveLength(1);
    const optimisticEx = optimistic[0];
    expect(optimisticEx.userEvent.type).toBe('MessageReceived');
    const optimisticKey = exchangeKey(optimisticEx);

    handleEvent(map, 'thread-1', 42, { type: 'MessageReceived', text: 'logo must match height' } as ThreadEvent, TS, EVENT_ID);
    expect(thread.pendingUserMessages).toHaveLength(0);

    const persisted = computeExchanges(thread);
    expect(persisted).toHaveLength(1);
    const persistedEx = persisted[0];

    expect(persistedEx.userSeq).not.toBe(optimisticEx.userSeq);
    expect(exchangeKey(persistedEx)).toBe(optimisticKey);
  });

  it('falls back to a unique seq-based key when the user event has no _eventId', () => {
    const a: Exchange = { userEvent: { type: 'MessageReceived', text: 'x' } as StoredEvent, userSeq: 3, steps: [] };
    const b: Exchange = { userEvent: { type: 'MessageReceived', text: 'y' } as StoredEvent, userSeq: 4, steps: [] };
    expect(exchangeKey(a)).not.toBe(exchangeKey(b));
  });

  it('never collides an _eventId key with a seq fallback key', () => {
    const withId: Exchange = { userEvent: { type: 'MessageReceived', text: 'x', _eventId: '5' } as StoredEvent, userSeq: 99, steps: [] };
    const withoutId: Exchange = { userEvent: { type: 'MessageReceived', text: 'y' } as StoredEvent, userSeq: 5, steps: [] };
    expect(exchangeKey(withId)).not.toBe(exchangeKey(withoutId));
  });
});
