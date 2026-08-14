import { describe, it, expect } from 'vitest';
import { TS, makeThreadState } from './thread-events-helpers';
import { abortPromisesAutoResume, abortTookEngineDown, computeExchanges, exchangeKey, exchangeStatus, groupIntoExchanges, handleEvent, isSwitchTeardownAbort, responseAbortedSummary, resumeEngineNote, continuableAbortIndex, type AbortCause, type Exchange, type MessageOrigin, type StoredEvent, type ThreadAggregate, type ThreadEvent, type ThreadState, type TransientEvent } from '../thread-events';

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
      blockingDescendantCount: 0, attentionDescendantCount: 0, liveEventWaitCount: 0,
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
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
    // aggregate says 'waiting' (e.g. coding_agent_proposed was set in the same exchange).
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

  // Regression (real thread 276f5580): a background WorktreeCleaned event is
  // pure bookkeeping (EventClass::Metadata in Rust, no render case). It must
  // NOT be folded as a step into whatever exchange happens to be `current` —
  // here the trailing ChildThreadCompleted card. The leak flipped the card's
  // status to a phantom 'coding-agent-working' that survived reloads.
  it('does not fold WorktreeCleaned into any exchange, leaving the child card "done"', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'spawn a child' }],
      [2, { type: 'CodingAgentTextStreamed', text: 'Spawned successfully' } as ThreadEvent],
      [3, { type: 'CodingAgentIdled', has_changes: false }],
      // Child finishes → completion card opens a new exchange on the parent.
      [4, { type: 'ChildThreadCompleted', child_thread_id: 'c1', status: 'success', summary: 'done' } as ThreadEvent],
      // An hour later the cleanup worker emits this onto the same thread.
      // WorktreeCleaned is now a first-class ThreadEvent variant (the
      // union-coverage contract test keeps it in sync with the generated
      // EVENT_CLASSIFICATION), so it constructs without a cast.
      [5, { type: 'WorktreeCleaned', tier: 0, freed_bytes: 15853 }],
    ]);
    const exchanges = groupIntoExchanges(events);
    // The WorktreeCleaned event belongs to no exchange.
    const allSteps = exchanges.flatMap(e => e.steps.map(s => s.event.type));
    expect(allSteps).not.toContain('WorktreeCleaned');
    // The child-completion card is the last exchange and has no steps…
    const card = exchanges[exchanges.length - 1];
    expect(card.userEvent.type).toBe('ChildThreadCompleted');
    expect(card.steps).toHaveLength(0);
    // …so it reads as terminal on an idle CC thread, not a phantom spinner.
    expect(
      exchangeStatus(card, '', /* isLast */ true, /* hasPriorActive */ false, /* threadIsCC */ true, /* threadIdle */ true),
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
    // Prior exchange keeps the abort as its terminating step (drives 'aborted' status)
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual(['TextStreamed', 'ResponseAborted']);
    // New boundary exchange wraps the AbortPanel
    expect(exchanges[1].userEvent.type).toBe('ResponseAborted');
    expect(exchanges[1].steps).toHaveLength(0);
  });

  it('splits at ContinuationStarted: opens a resume exchange that absorbs the engine note', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix the bug' }],
      [2, { type: 'ResponseAborted', text: '' }],
      [3, { type: 'ContinuationStarted' }],
      // Engine-mode UserPromptInjected (the engine note) — must absorb into the
      // ContinuationStarted exchange as a step, not start a new exchange.
      [4, { type: 'UserPromptInjected', text: '[Engine note]', mode: 'engine' }],
      [5, { type: 'TextStreamed', text: 'On it.' }],
      [6, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[1].userEvent.type).toBe('ResponseAborted'); // boundary
    expect(exchanges[2].userEvent.type).toBe('ContinuationStarted');
    expect(exchanges[2].steps.map(s => s.event.type)).toEqual([
      'UserPromptInjected', 'TextStreamed', 'ResponseGenerated',
    ]);
  });

  it('Human-mode UserPromptInjected after ContinuationStarted does NOT absorb (legacy correction)', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'ContinuationStarted' }],
      [2, { type: 'UserPromptInjected', text: 'human correction', mode: 'human' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    // Two exchanges: ContinuationStarted, then UserPromptInjected as its own boundary.
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].userEvent.type).toBe('ContinuationStarted');
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

  // Regression for the off-by-one rendering observed in real thread
  // 9b5a05aa: when the orphan-injection re-process loop stamped A's req_id
  // onto a turn whose response actually belonged to B, the events the chat
  // loop emits BEFORE its first injection check (the memory recall + the
  // per-call ContextCaptured) used to bypass request-id routing and leak into
  // B's exchange. The leaked step flipped the status heuristic to 'aborted'
  // (the "stale exchange" branch needs hasSteps=true). Route every chat-loop
  // event that carries request_event_id back to its anchor — none should
  // fall through to `current`. `ContextCaptured` and `MemoryRecalled` are the
  // live events; `ContextAssembled`/`ContextTokensMeasured`/`MemorySearched`
  // are their retired predecessors, still routed for legacy DB rows.
  it('routes ContextCaptured/MemoryRecalled (+ legacy predecessors) by request_event_id, not current pointer', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'A', _eventId: 'A' }],
      [2, { type: 'TextStreamed', text: 'a-reply', request_event_id: 'A' } as StoredEvent],
      [3, { type: 'ResponseGenerated', text: 'a-reply', request_event_id: 'A' } as StoredEvent],
      // B opens a new exchange in the timeline. Every late event for A's loop
      // (including the chat-loop preludes) lands AFTER B's MR in the seq stream.
      [4, { type: 'MessageReceived', text: 'B', _eventId: 'B' }],
      [5, { type: 'MemoryRecalled', queries: [], results: 0, request_event_id: 'A' } as unknown as StoredEvent],
      [6, { type: 'ContextCaptured', producer: 'chat', model: 'm', context_window: 0, sections: [], tools: [], estimated_total_tokens: 0, request_event_id: 'A' } as unknown as StoredEvent],
      // Retired predecessors — still routed so legacy threads behave.
      [7, { type: 'ContextAssembled', sections: [], tools: [], model: 'm', total_chars: 0, request_event_id: 'A' } as unknown as StoredEvent],
      [8, { type: 'ContextTokensMeasured', tokens: 100, message_count: 1, request_event_id: 'A' } as unknown as StoredEvent],
      [9, { type: 'MemorySearched', queries: [], results: 0, request_event_id: 'A' } as unknown as StoredEvent],
      [10, { type: 'TextStreamed', text: 'late', request_event_id: 'A' } as StoredEvent],
      [11, { type: 'ResponseGenerated', text: 'late', request_event_id: 'A' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);

    expect((exchanges[0].userEvent as { text: string }).text).toBe('A');
    // Every late event with req=A routes back to A, including the
    // chat-loop preludes that previously fell through to `current`.
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
    // B has no events of its own — none stamped with req=B in this scenario.
    expect(exchanges[1].steps).toHaveLength(0);
  });

  // End-to-end regression for real thread ad178d6a: the user sent a follow-up
  // ("hmm?") while A's response was mid-stream, then A's `ContextCaptured`
  // (carrying A's req_id) arrived AFTER B's MR. Before the fix it fell through
  // to `current` (= B's empty exchange), giving B a lone step; in the brief
  // idle window before B's own response started, exchangeStatus read
  // "isLast + hasSteps + !isComplete + threadIdle" as a stale crash and
  // flashed 'aborted' — which then flipped to 'streaming'/'Working' once B's
  // events landed. The user saw "Aborted → Working" on the follow-up. With
  // ContextCaptured routed by req_id, B stays empty and reads 'done' at idle.
  it('does not flash "aborted" on a follow-up when the prior response\'s ContextCaptured arrives late', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'Min side > Innboks > Kontakt oss?', _eventId: 'A' }],
      [2, { type: 'MemorySearched', queries: [], results: [], request_event_id: 'A' } as unknown as StoredEvent],
      [3, { type: 'ThoughtStreamed', text: '', request_event_id: 'A' } as unknown as StoredEvent],
      // User fires the follow-up while A is still streaming.
      [4, { type: 'MessageReceived', text: 'hmm?', _eventId: 'B' }],
      // A's per-call ContextCaptured lands AFTER B's MR — the leaker.
      [5, { type: 'ContextCaptured', producer: 'chat', model: 'm', context_window: 0, sections: [], tools: [], estimated_total_tokens: 0, request_event_id: 'A' } as unknown as StoredEvent],
      [6, { type: 'TextStreamed', text: 'a-reply', request_event_id: 'A' } as StoredEvent],
      [7, { type: 'ResponseGenerated', text: 'a-reply', request_event_id: 'A' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);

    // ContextCaptured routes back to A, not the follow-up.
    expect((exchanges[0].userEvent as { text: string }).text).toBe('Min side > Innboks > Kontakt oss?');
    expect(exchanges[0].steps.map(s => s.event.type)).toContain('ContextCaptured');

    // The follow-up exchange is empty — no leaked step.
    const followUp = exchanges[1];
    expect((followUp.userEvent as { text: string }).text).toBe('hmm?');
    expect(followUp.steps).toHaveLength(0);

    // In the idle window before B's own response starts, the follow-up reads
    // 'done' (empty-idle), never 'aborted'.
    expect(
      exchangeStatus(followUp, '', /* isLast */ true, /* hasPriorActive */ false, /* threadIsCC */ false, /* threadIdle */ true),
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

  // Claude Code sessions reuse one `request_event_id` across mid-flight follow-ups (the
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
    // 1: A's CC reply, 2: B follow-up (with ResponseCanceled step), 3: cancel boundary panel
    expect(exchanges).toHaveLength(3);
    expect(exchanges[1].steps.map(s => s.event.type)).toContain('ResponseCanceled');
    expect(exchanges[2].userEvent.type).toBe('ResponseCanceled');
  });

  // Codex mid-turn follow-up redirect: the interrupted turn emits
  // ResponseCanceled(superseded_by_followup). Unlike a real Stop cancel, this
  // must NOT open a standalone "Response canceled" boundary exchange — the turn
  // renders neutrally (like the chat/CC follow-up). The cancel stays a step on
  // the originating turn so step resolution still sees a terminator.
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
    // 1: A's interrupted turn (carries the cancel + idle as steps), 2: B follow-up.
    // No third "Response canceled" boundary panel.
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].steps.map(s => s.event.type)).toContain('ResponseCanceled');
    expect(exchanges.some(e => e.userEvent.type === 'ResponseCanceled')).toBe(false);
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
    // 1: A's chat exchange (with ResponseCanceled step), 2: B's exchange,
    // 3: cancel boundary panel
    expect(exchanges).toHaveLength(3);
    expect(exchanges[0].steps.map(s => s.event.type)).toContain('ResponseCanceled');
    expect(exchanges[1].steps.map(s => s.event.type)).not.toContain('ResponseCanceled');
    expect(exchanges[2].userEvent.type).toBe('ResponseCanceled');
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

describe('continuableAbortIndex', () => {
  it('returns the latest aborted exchange when no ContinuationStarted follows', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted' }],
      [3, { type: 'MessageReceived', text: 'two' }],
      [4, { type: 'ResponseAborted' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    // 4 exchanges: msg, abort, msg, abort
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

  // Stale-settle aborts are engine cleanup of stuck threads — clicking
  // Continue would re-run work the user just stopped. Treat them like
  // ContinuationStarted so the AbortPanel renders without a Continue button.
  it('returns null when the latest abort is stale-settle', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted', cause: 'stale_settle' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBeNull();
  });

  it('returns null when a stale-settle abort sits above an older real abort', () => {
    // The older real abort is irrelevant once the user has triggered
    // stale-settle on the thread — the thread is settled, no Continue.
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted', cause: 'engine_shutdown' }],
      [3, { type: 'MessageReceived', text: 'two' }],
      [4, { type: 'ResponseAborted', cause: 'stale_settle' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBeNull();
  });

  // The switch fingerprint, mirroring the backend's SWITCH_TEARDOWN_ABORT_SQL:
  // engine_shutdown AND a device actor. The engine auto-resumes that turn, so
  // offering Continue races its own recovery (the 2026-08-05 report: the button
  // sat there through the whole restart and the user's click landed first).
  it('returns null on a switch-teardown abort: the engine is auto-resuming it', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, {
        type: 'ResponseAborted',
        cause: 'engine_shutdown',
        actor: { kind: 'device', device_id: 'd1', label: 'My iPhone' },
      }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBeNull();
  });

  // Both halves of the fingerprint are load-bearing on the backend, so both are
  // here. A bare engine_shutdown (stop.sh / an external SIGUSR1) is NOT a switch:
  // nothing auto-resumes it, so it keeps the manual Continue.
  it('offers Continue on an engine_shutdown abort with no device actor', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted', cause: 'engine_shutdown', actor: { kind: 'system' } }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBe(1);
  });

  // The other half: a device actor on a NON-shutdown cause is not a switch
  // either. `stale_settle` deliberately carries the actor of the button that
  // exposed the stuck row, which is why an actor-only check would be wrong.
  it('offers Continue on the boot floor abort that withdraws a resume promise', () => {
    // The engine declined to resume this switch-interrupted thread, so the boot
    // floor emitted a fresh recovery_after_restart abort on top. That newer
    // abort is not a switch abort, so the button comes back.
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, {
        type: 'ResponseAborted',
        cause: 'engine_shutdown',
        actor: { kind: 'device', device_id: 'd1', label: 'My iPhone' },
      }],
      [3, { type: 'ResponseAborted', cause: 'recovery_after_restart', actor: { kind: 'system' } }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBe(2);
  });

  // An abort boundary can ACQUIRE a turn. An event-wait delivery anchors on an
  // event that is not an exchange-start type, so its whole turn folds into
  // whatever boundary is current, and if that boundary is an abort the turn
  // renders under it. Continue there re-runs completed work: on 2026-08-06 the
  // button sat above a turn that had applied a change and spawned a sub-thread
  // two minutes earlier (real thread ebc787a4).
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

  // A resolved boundary does not STOP the scan: an older unresolved abort above
  // it is still legitimately continuable, so the walk keeps going rather than
  // returning null the way ContinuationStarted does.
  it('skips a resolved abort and offers the older unresolved one', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted', cause: 'safety_net' }],
      [3, { type: 'MessageReceived', text: 'two' }],
      [4, { type: 'ResponseAborted', cause: 'safety_net' }],
      [5, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    // 0: msg, 1: abort (unresolved), 2: msg, 3: abort (resolved by [5]).
    expect(continuableAbortIndex(exchanges)).toBe(1);
  });

  // The ordinary shape stays untouched: a bare boundary with no work under it
  // is exactly what Continue exists for.
  it('still offers Continue on an abort with steps but no terminal', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted', cause: 'safety_net' }],
      [3, { type: 'TextStreamed', text: 'partial, then the engine died again' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBe(1);
  });

  // Crash recovery emits the boundary and its OWN marker together: a
  // `recovery_after_restart` abort immediately followed by the synthetic
  // `CodingAgentIdled { reason: engine_restart_interrupt }` whose entire purpose
  // is to say "this was interrupted, offer Continue"
  // (`agent_recovery/recovery.rs`). `CodingAgentIdled` is not an exchange-start
  // type, so it folds into the abort as a step, and the resolved-boundary check
  // read the engine's own offer as a turn that had run and finished. Every
  // coding-agent thread a restart touched came back unresumable (reported
  // 2026-08-07, across a whole workspace at once).
  it('offers Continue on the recovery pair, whose idle IS the interruption marker', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'one' }],
      [2, { type: 'ResponseAborted', cause: 'recovery_after_restart', actor: { kind: 'system' } }],
      [3, { type: 'CodingAgentIdled', reason: 'engine_restart_interrupt' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(continuableAbortIndex(exchanges)).toBe(1);
  });

  // The other direction, and the reason the carve-out is keyed on the reason
  // rather than on the event type: a coding-agent turn that genuinely ran under
  // the boundary and finished still resolves it.
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
});

describe('abortPromisesAutoResume', () => {
  const device = { kind: 'device', device_id: 'd1', label: 'My iPhone' } as const;

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
    // Legacy rows carry neither field.
    expect(abortPromisesAutoResume({ type: 'ResponseAborted' })).toBe(false);
    expect(abortPromisesAutoResume({ type: 'ResponseGenerated' })).toBe(false);
  });

  /** The Continue button and the transcript label read the same fingerprint, so
   *  a turn can never say "Paused by restart" while offering the button that
   *  means nothing is resuming it (or the reverse). Both route through
   *  `isSwitchTeardownAbort`; this pins that they still agree, in both
   *  directions, across the shapes that differ in only one half of the pair. */
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

  /** `stale_settle` carries the actor of whichever button exposed the stuck row
   *  (Stop / Apply / Discard / Archive), so a device actor there must NOT read as
   *  a switch. It gets its own label, and the engine settles it to idle rather
   *  than to any verdict. */
  it('never reads a device-attributed stale_settle as a switch', () => {
    expect(isSwitchTeardownAbort(device, 'stale_settle')).toBe(false);
    expect(responseAbortedSummary(device, 'stale_settle')).toBe('Settled stuck response');
  });

  /** The two predicates answer different questions and differ by exactly the
   *  actor: `abortTookEngineDown` asks whether the engine is gone (so nothing
   *  can be running), `abortPromisesAutoResume` asks whether it promised to
   *  bring the turn back (so the button and the wording change).
   *
   *  The row that matters is the unattributed shutdown, which is every terminal
   *  `stop.sh`, external SIGUSR1 and ctrl-c: engine down, nothing promised. It
   *  read as neither before 2026-08-13, so the boundary kept a shimmering
   *  "Working" over a dead subprocess (real thread b146c294). */
  it('separates "the engine is gone" from "the engine promised to resume"', () => {
    const cases: { cause?: AbortCause; actor?: MessageOrigin; down: boolean; promised: boolean }[] = [
      { cause: 'engine_shutdown', actor: device, down: true, promised: true },
      { cause: 'engine_shutdown', actor: { kind: 'system' }, down: true, promised: false },
      { cause: 'engine_shutdown', down: true, promised: false },
      // The engine is alive for every other cause, so its boundary can still
      // acquire a live turn (real thread ebc787a4, the `safety_net` case).
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
    // Only an abort qualifies: no other terminator takes the engine with it.
    expect(abortTookEngineDown({ type: 'ResponseGenerated' })).toBe(false);
    expect(abortTookEngineDown({ type: 'ResponseCanceled', cause: 'user_stop' })).toBe(false);
  });
});

describe('resumeEngineNote', () => {
  it('reads the engine note from a ContinuationStarted exchange and counts tool bullets', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'ContinuationStarted' }],
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

  it('returns null when the ContinuationStarted has no engine UserPromptInjected step', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'ContinuationStarted' }],
      // CC resume path emits ContinuationStarted alone (no engine note).
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
    expect(thread.events.get(1)).toEqual(expect.objectContaining(event1)); // original kept
  });

  it('takes the latest cumulative transient text into the streaming buffer', () => {
    const thread = makeThreadState();
    const threadMap = new Map([['thread-1', thread]]);
    // Each `CumulativeTextUpdated` carries the whole accumulated turn text, not
    // a delta, so the later frame replaces the earlier one.
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

    // A ToolCalled event arrives — should NOT clear pending messages
    const result = handleEvent(threadMap, 'thread-1', 100, { type: 'ToolCalled', name: 'search', args: {} }, TS);

    // pendingUserMessages should still be there
    expect(thread.pendingUserMessages).toHaveLength(1);
    expect(result.clearedPendingUserMessage).toBe(false);

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

  it('does not hide a removed message once UserPromptInjected already attached', () => {
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
      type: 'UserPromptInjected',
      text: 'queued',
      mode: 'human',
      injected_message_id: 'msg-2',
    } as ThreadEvent, TS, 'inject-1');

    const exchanges = computeExchanges(thread);
    expect(exchanges).toHaveLength(2);
    expect((exchanges[1].userEvent as Extract<ThreadEvent, { type: 'MessageReceived' }>).text).toBe('queued');
    expect(exchanges[1].steps.map(step => step.event.type)).toEqual(['UserPromptInjected']);
  });

  it('clears matching pendingUserMessage on UserPromptInjected with matching event_id', () => {
    const thread = makeThreadState();
    thread.pendingUserMessages = [{ text: 'fix the bug', eventId: 'inject-1', created: '2026-01-01T00:00:00Z' }];
    const threadMap = new Map([['thread-1', thread]]);

    const result = handleEvent(threadMap, 'thread-1', 10, { type: 'UserPromptInjected', text: 'fix the bug' }, TS, 'inject-1');
    expect(thread.pendingUserMessages).toEqual([]);
    expect(result.clearedPendingUserMessage).toBe(true);
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

    const result = handleEvent(threadMap, 'thread-1', 10, {
      type: 'UserQuestionAnswered',
      tool_use_id: 'tu-1',
      answer: { kind: 'FreeText', text: 'Ask anyway but proceed if reversible' },
    } as ThreadEvent, TS);

    expect(thread.pendingUserMessages).toEqual([]);
    expect(result.clearedPendingUserMessage).toBe(true);
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

// ===========================================================================
// exchangeKey — stable render identity across the optimistic→persisted swap
// ===========================================================================
describe('exchangeKey', () => {
  it('keeps the same key when an optimistic pending message becomes a persisted MessageReceived', () => {
    // Reproduces the "follow-up disappears, then shows up after a while" bug.
    // The optimistic synthetic exchange sorts at a MAX_SAFE_INTEGER seq; the
    // persisted MessageReceived carries its real (much smaller) DB seq. Keying
    // the rendered <ChatExchange> by `userSeq` therefore changes the key on the
    // swap, remounting the DOM node — which churns the auto-scroll observers and
    // leaves the message off-screen until a later event re-snaps to the bottom.
    // The client `event_id` round-trips as the events-table PK, so `_eventId` is
    // identical across the swap and is the stable identity to key on.
    const thread = makeThreadState();
    const EVENT_ID = 'client-uuid-1';
    thread.pendingUserMessages.push({ text: 'logo must match height', eventId: EVENT_ID, created: TS });
    const map = new Map([['thread-1', thread]]);

    // Optimistic phase.
    const optimistic = computeExchanges(thread);
    expect(optimistic).toHaveLength(1);
    const optimisticEx = optimistic[0];
    expect(optimisticEx.userEvent.type).toBe('MessageReceived');
    const optimisticKey = exchangeKey(optimisticEx);

    // Server confirmation: real MessageReceived arrives via SSE with the SAME
    // client event_id but a real DB seq. handleEvent clears the optimistic row.
    handleEvent(map, 'thread-1', 42, { type: 'MessageReceived', text: 'logo must match height' } as ThreadEvent, TS, EVENT_ID);
    expect(thread.pendingUserMessages).toHaveLength(0);

    const persisted = computeExchanges(thread);
    expect(persisted).toHaveLength(1);
    const persistedEx = persisted[0];

    // The seq genuinely changes — the old `'ex-' + userSeq` key would remount.
    expect(persistedEx.userSeq).not.toBe(optimisticEx.userSeq);
    // …but the stable key does not, so Preact reconciles the node in place.
    expect(exchangeKey(persistedEx)).toBe(optimisticKey);
  });

  it('falls back to a unique seq-based key when the user event has no _eventId', () => {
    const a: Exchange = { userEvent: { type: 'MessageReceived', text: 'x' } as StoredEvent, userSeq: 3, steps: [] };
    const b: Exchange = { userEvent: { type: 'MessageReceived', text: 'y' } as StoredEvent, userSeq: 4, steps: [] };
    expect(exchangeKey(a)).not.toBe(exchangeKey(b));
  });

  it('never collides an _eventId key with a seq fallback key', () => {
    // A seq fallback could otherwise stringify to the same value as an _eventId.
    const withId: Exchange = { userEvent: { type: 'MessageReceived', text: 'x', _eventId: '5' } as StoredEvent, userSeq: 99, steps: [] };
    const withoutId: Exchange = { userEvent: { type: 'MessageReceived', text: 'y' } as StoredEvent, userSeq: 5, steps: [] };
    expect(exchangeKey(withId)).not.toBe(exchangeKey(withoutId));
  });
});
