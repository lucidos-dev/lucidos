import { describe, it, expect, beforeEach } from 'vitest';
import { getExchanges, getLabel, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { exchangeResponseEvents, exchangeResponseText, exchangeStatus, exchangeSteps, exchangeUserMessage, isEmptyContinuedExchange, renderOrderViolations, resumeEngineNote, type ThreadEvent } from '../thread-events';
import { hidesEarlierProse } from '../event-rendering';

beforeEach(resetSeqCounter);

describe('Flow: Interrupted exchanges', () => {
  it('CC exchange followed by another shows "Done"', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix bug', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:00:02Z' },
      { type: 'MessageReceived', text: 'Actually also fix tests', created: '2026-01-01T00:01:00Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:01:01Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false)).toBe('interrupted');
    expect(getLabel(exchanges[0], '', false)).toBe('Done');
    expect(exchangeStatus(exchanges[1], '', true)).toBe('done');
  });

  it('CC exchange that went idle then got follow-up shows Done, not interrupted', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-01-01T00:00:03Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:04Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:05Z' },
      { type: 'MessageReceived', text: 'Now fix tests too', created: '2026-01-01T00:01:00Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:01:01Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    expect(getLabel(exchanges[0], '', false)).toBe('Done');
  });

  it('CC exchange that ended (SessionEnded) then got follow-up shows Done', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:00:02Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:03Z' },
      { type: 'SessionEnded', created: '2026-01-01T00:00:04Z' },
      { type: 'MessageReceived', text: 'Something else', created: '2026-01-01T00:01:00Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:01:01Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    expect(getLabel(exchanges[0], '', false)).toBe('Done');
  });

  it('CC follow-up exchange (no own SessionStarted) interrupted shows "Done"', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Center the separator', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-01-01T00:00:03Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:04Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:05Z' },
      { type: 'MessageReceived', text: 'they are still a bit much', created: '2026-01-01T00:01:00Z' },
      { type: 'MessageReceived', text: 'sorry wrong thread', created: '2026-01-01T00:02:00Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:02:01Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    expect(exchangeStatus(exchanges[1], '', false, false, true)).toBe('done');
    expect(exchangeStatus(exchanges[2], '', true)).toBe('done');
  });

  it('CC exchange with only SessionStarted (no body events) before follow-up: interrupted with no visible events', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'so now u can use gh correctly?', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-01-01T00:00:02Z' },
      { type: 'MessageReceived', text: 'and git?', created: '2026-01-01T00:00:05Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false, false, true)).toBe('interrupted');
    expect(getLabel(exchanges[0], '', false, false, true)).toBe('Done');
    expect(exchangeResponseEvents(exchanges[0],  false)).toEqual([]);
    expect(exchangeResponseText(exchanges[0])).toBe('');
  });

  it('CC exchange with SessionStarted + Thinking only before follow-up: panel is empty-continued', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'What does that mean?', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-01-01T00:00:01Z' },
      { type: 'ThoughtStreamed', text: '', created: '2026-01-01T00:00:02Z' },
      { type: 'MessageReceived', text: 'follow-up', created: '2026-01-01T00:01:00Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false, false, true)).toBe('interrupted');

    const events = exchangeResponseEvents(exchanges[0]);
    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({ type: 'step', description: 'Thinking' });

    expect(isEmptyContinuedExchange('interrupted', false, events, false)).toBe(true);
  });

  it('CC follow-up with whitespace-only CodingAgentTextStreamed + CodingAgentPromptSent before next follow-up: panel is empty-continued', () => {
    const { map, id } = makeThread();

    const reqId = 'cc-session-req-id';
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'What?? Running???', created: '2026-01-01T00:00:00.000Z' },
      { type: 'CodingAgentTextStreamed', text: '\n\n', request_event_id: reqId, created: '2026-01-01T00:00:00.020Z' } as any,
      { type: 'CodingAgentPromptSent', text: 'What?? Running???', request_event_id: reqId, created: '2026-01-01T00:00:00.030Z' } as any,
      { type: 'MessageReceived', text: 'There should be no Archive btn...', created: '2026-01-01T00:01:00.000Z' },
      { type: 'CodingAgentPromptSent', text: 'There should be no Archive btn...', request_event_id: reqId, created: '2026-01-01T00:01:00.010Z' } as any,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false, false, true)).toBe('interrupted');

    const events = exchangeResponseEvents(exchanges[0]);
    expect(isEmptyContinuedExchange('interrupted', false, events, false)).toBe(true);
  });

  it('regular (non-CC) exchange followed by another shows Done, not interrupted', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'First', created: '2026-01-01T00:00:00Z' },
      { type: 'TextStreamed', text: 'Answer 1', created: '2026-01-01T00:00:01Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:02Z' },
      { type: 'MessageReceived', text: 'Second', created: '2026-01-01T00:01:00Z' },
      { type: 'TextStreamed', text: 'Answer 2', created: '2026-01-01T00:01:01Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:01:02Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    expect(getLabel(exchanges[0], '', false)).toBe('Done');
  });

  it('chat exchange interrupted by mid-flight PromptInjected shows interrupted, not Working', () => {
    const { map, id } = makeThread();
    const mr1Id = 'mr1-event-id';
    const mr2Id = 'mr2-event-id';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Organize nodes on same level...', created: '2026-01-01T00:00:00Z', event_id: mr1Id },
      { type: 'MemorySearched', request_event_id: mr1Id, results: [], query: 'q', created: '2026-01-01T00:00:02Z' } as any,
      { type: 'ThoughtStreamed', request_event_id: mr1Id, text: 'thinking...', created: '2026-01-01T00:00:03Z' } as any,
      { type: 'ToolCalled', name: 'Read', args: {}, request_event_id: mr1Id, created: '2026-01-01T00:00:10Z' } as any,
      { type: 'ToolResult', name: 'Read', result: 'ok', request_event_id: mr1Id, created: '2026-01-01T00:00:11Z' } as any,
      { type: 'MessageReceived', text: 'Also use more horizontal space...', created: '2026-01-01T00:00:30Z', event_id: mr2Id },
      { type: 'PromptInjected', text: 'Also use more horizontal space...', mode: 'human',
        request_event_id: mr1Id, injected_message_id: mr2Id, created: '2026-01-01T00:01:00Z' } as any,
      { type: 'ThoughtStreamed', request_event_id: mr1Id, text: 'more thinking', created: '2026-01-01T00:01:05Z' } as any,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false)).toBe('interrupted');
    expect(getLabel(exchanges[0], '', false)).toBe('Done');
    expect(exchangeStatus(exchanges[1], '', true)).toBe('streaming');
    expect(getLabel(exchanges[1], '', true)).toBe('Working');
  });
});

describe('Flow: Recovery Claude Code session', () => {
  it('recovery session with tools and idle shows done with change panel data', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'A previous Claude Code session was interrupted...' },
      { type: 'SessionStarted', session_id: 'recovery-1' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'file contents' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'CodingAgentTextStreamed', text: 'I finished the cleanup.' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    expect(getLabel(exchanges[0])).toBe('Done');
    expect(exchangeResponseText(exchanges[0])).toContain('I finished the cleanup.');
    expect(exchangeSteps(exchanges[0]).length).toBe(2);
  });

  it('recovery session that completes without changes shows Done', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Recovery...' },
      { type: 'SessionStarted', session_id: 'recovery-1' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'nothing to do' },
      { type: 'CodingAgentTextStreamed', text: 'Nothing to clean up.' },
      { type: 'ResponseGenerated' },
      { type: 'SessionEnded' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    expect(getLabel(exchanges[0])).toBe('Done');
  });
});

describe('Flow: ContinuationStarted recovery', () => {
  it('ContinuationStarted acts as exchange boundary, thread shows CC content', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'ContinuationStarted', branch: 'claude-code/20260318-122816' },
      { type: 'SessionStarted', session_id: 'cc-1', branch: 'claude-code/20260318-122816' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' },
      { type: 'CodingAgentTextStreamed', text: 'Reviewed and continuing.' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].userEvent.type).toBe('ContinuationStarted');
    expect(exchangeUserMessage(exchanges[0])).toBe('Resumed after engine restart');
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    expect(exchangeResponseText(exchanges[0])).toContain('Reviewed and continuing.');
  });

  it('ContinuationStarted from user-clicked Continue does NOT say "engine restart"', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      {
        type: 'ContinuationStarted',
        branch: 'claude-code/20260318-122816',
        actor: { kind: 'device', device_id: 'd-1' },
      },
      { type: 'SessionStarted', session_id: 'cc-2', branch: 'claude-code/20260318-122816' },
      { type: 'CodingAgentTextStreamed', text: 'Continuing.' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].userEvent.type).toBe('ContinuationStarted');
    expect(exchangeUserMessage(exchanges[0])).toBe('Continued the response');
    expect(exchangeUserMessage(exchanges[0])).not.toContain('engine restart');
  });

  it('ContinuationStarted from api actor renders as user continuation too', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      {
        type: 'ContinuationStarted',
        branch: 'claude-code/20260318',
        actor: { kind: 'api', user_agent: 'lucidos-cli/0.1' },
      },
      { type: 'SessionStarted', session_id: 'cc-3' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled', has_changes: false },
    ]);
    const exchanges = getExchanges(map, id);
    expect(exchangeUserMessage(exchanges[0])).toBe('Continued the response');
  });

  it('ContinuationStarted from api actor with engine mode keeps the engine-restart label', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      {
        type: 'ContinuationStarted',
        branch: 'claude-code/20260318',
        actor: { kind: 'api', mode: 'engine', user_agent: 'engine-internal' },
      },
      { type: 'SessionStarted', session_id: 'cc-4' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled', has_changes: false },
    ]);
    const exchanges = getExchanges(map, id);
    expect(exchangeUserMessage(exchanges[0])).toBe('Resumed after engine restart');
  });

  it('ContinuationStarted in existing thread (with prior messages) creates new exchange', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 'cc-orig', branch: 'claude-code/20260318-122816' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled', has_changes: true },
      { type: 'ContinuationStarted', branch: 'claude-code/20260318-122816' },
      { type: 'SessionStarted', session_id: 'cc-recovery', branch: 'claude-code/20260318-122816' },
      { type: 'CodingAgentTextStreamed', text: 'Continuing work.' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[1].userEvent.type).toBe('ContinuationStarted');
    expect(exchangeStatus(exchanges[1], '', true)).toBe('done');
  });

  it('ContinuationStarted that completes with SessionEnded shows Done', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'ContinuationStarted', branch: 'claude-code/20260318' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentTextStreamed', text: 'Nothing to do.' },
      { type: 'ResponseGenerated' },
      { type: 'SessionEnded' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
  });
});

describe('Flow: Edge cases', () => {
  it('pendingUserMessages cleared, backend MessageReceived groups events correctly', () => {
    const { map, id } = makeThread();
    map.get(id)!.pendingUserMessages = [{ text: 'My question', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }];

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'My question' },
      { type: 'ToolCalled', name: 'search', args: {} },
      { type: 'ToolResult', name: 'search', result: 'found' },
      { type: 'TextStreamed', text: 'Here is the answer.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeUserMessage(exchanges[0])).toBe('My question');
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    expect(exchangeResponseText(exchanges[0])).toBe('Here is the answer.');
    expect(exchangeSteps(exchanges[0])).toHaveLength(1);
    expect(getLabel(exchanges[0])).toBe('Done');
  });

  it('multiple tool calls all show in steps', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Complex task' },
      { type: 'ToolCalled', name: 'read_file', args: {} },
      { type: 'ToolResult', name: 'read_file', result: 'contents' },
      { type: 'ToolCalled', name: 'web_search', args: {} },
      { type: 'ToolResult', name: 'web_search', result: 'results' },
      { type: 'ToolCalled', name: 'write_file', args: {} },
      { type: 'ToolResult', name: 'write_file', result: 'ok' },
      { type: 'TextStreamed', text: 'All done.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    const steps = exchangeSteps(exchanges[0]);
    expect(steps).toHaveLength(3);
    expect(steps[0].description).toBe('Read file');
    expect(steps[1].description).toBe('Web search');
    expect(steps[2].description).toBe('Write file');
    expect(steps.every(s => s.outcome === 'success')).toBe(true);
  });

  it('ToolCalled without ToolResult shows pending step', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Do something' },
      { type: 'ToolCalled', name: 'slow_tool', args: {} },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeSteps(exchanges[0])).toHaveLength(1);
    expect(exchangeSteps(exchanges[0])[0].outcome).toBe('pending');
    expect(getLabel(exchanges[0])).toBe('Working');
  });

  it('Thinking event creates a step with context metadata', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Hello' },
      { type: 'ThoughtStreamed', text: 'Context: 5000 tokens, 3 messages', context_tokens: 5000, context_messages: 3 },
      { type: 'TextStreamed', text: 'Hi!' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    const steps = exchangeSteps(exchanges[0]);
    expect(steps).toHaveLength(1);
    expect(steps[0].description).toBe('Thinking');
    expect(steps[0].outcome).toBe('success');
    expect(steps[0].context_tokens).toBe(5000);
    expect(steps[0].context_messages).toBe(3);

    const events = exchangeResponseEvents(exchanges[0]);
    const stepEvents = events.filter(e => e.type === 'step');
    expect(stepEvents).toHaveLength(1);
    expect((stepEvents[0] as { description: string }).description).toBe('Thinking');
  });

  const recallTurn = (type: 'MemoryRecalled' | 'MemorySearched', results: number) => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'What is my birthday?' },
      { type, results, queries: ['birthday', 'date of birth'] } as ThreadEvent,
      { type: 'ThoughtStreamed', text: 'Context: 2000 tokens, 2 messages', context_tokens: 2000, context_messages: 2 },
      { type: 'TextStreamed', text: 'Jan 1.' },
      { type: 'ResponseGenerated' },
    ]);
    const exchange = getExchanges(map, id)[0];
    return {
      steps: exchangeSteps(exchange),
      stepEvents: exchangeResponseEvents(exchange).filter(e => e.type === 'step'),
    };
  };

  it('MemoryRecalled event creates a step labelled with the count', () => {
    const { steps, stepEvents } = recallTurn('MemoryRecalled', 12);

    expect(steps).toHaveLength(2);
    expect(steps[0].description).toBe('Recalled 12 memories');
    expect(steps[0].outcome).toBe('success');
    expect(steps[1].description).toBe('Thinking');

    expect(stepEvents).toHaveLength(2);
    expect((stepEvents[0] as { recall?: unknown }).recall)
      .toEqual({ memories: [], queries: ['birthday', 'date of birth'] });
  });

  it('a recall step carries the memories the engine injected, in order', () => {
    const { map, id } = makeThread();
    const memories = [
      {
        id: 'm1', topic: 'family', summary: 'Birthday is January 1',
        src_created_at: '2026-01-02T03:04:05Z', source: { type: 'event', id: 'e1' },
      },
      {
        id: 'm2', topic: 'notes', summary: 'Party plan',
        src_created_at: '2026-01-03T03:04:05Z', source: { type: 'artifact', path: 'party.md', commit: 'abc' },
      },
    ];
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'What is my birthday?' },
      { type: 'MemoryRecalled', results: 2, queries: ['birthday'], memories } as ThreadEvent,
      { type: 'TextStreamed', text: 'Jan 1.' },
      { type: 'ResponseGenerated' },
    ]);
    const step = exchangeResponseEvents(getExchanges(map, id)[0]).find(e => e.type === 'step');
    expect(step).toMatchObject({ description: 'Recalled 2 memories', recall: { memories, queries: ['birthday'] } });
    expect((step as { detail?: string }).detail).toBeUndefined();
  });

  it('a recall step with nothing to list carries no recall', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'hi' },
      { type: 'MemoryRecalled', results: 0 } as ThreadEvent,
      { type: 'ResponseGenerated' },
    ]);
    const step = exchangeResponseEvents(getExchanges(map, id)[0]).find(e => e.type === 'step');
    expect((step as { recall?: unknown }).recall).toBeUndefined();
  });

  it('recall step label covers zero, one and many, and never says "search"', () => {
    expect(recallTurn('MemoryRecalled', 0).steps[0].description).toBe('No memories recalled');
    expect(recallTurn('MemoryRecalled', 1).steps[0].description).toBe('Recalled 1 memory');
    expect(recallTurn('MemoryRecalled', 12).steps[0].description).toBe('Recalled 12 memories');
    for (const n of [0, 1, 12]) {
      expect(recallTurn('MemoryRecalled', n).steps[0].description.toLowerCase()).not.toContain('search');
    }
  });

  it('legacy MemorySearched rows render as a recall step', () => {
    const clockless = <T extends object>(rows: T[]) => rows.map((row) => ({ ...row, seq: undefined }));
    for (const n of [0, 1, 12]) {
      expect(recallTurn('MemorySearched', n).steps).toEqual(recallTurn('MemoryRecalled', n).steps);
      expect(clockless(recallTurn('MemorySearched', n).stepEvents))
        .toEqual(clockless(recallTurn('MemoryRecalled', n).stepEvents));
    }
  });

  it('ToolResult content is attached to the matching step (LLM tools)', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'List files' },
      { type: 'ToolCalled', name: 'list_files', args: { path: '.' } },
      { type: 'ToolResult', name: 'list_files', result: 'a.txt\nb.txt' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    const stepEvents = exchangeResponseEvents(exchanges[0]).filter(e => e.type === 'step') as Array<{ description: string; result?: string }>;
    expect(stepEvents).toHaveLength(1);
    expect(stepEvents[0].result).toBe('a.txt\nb.txt');
  });

  it('CodingAgentToolResult content is paired by tool_use_id and attached to the matching step', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Run a command' },
      { type: 'CodingAgentUserMessageSent', text: 'Run a command' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'ls' }, tool_use_id: 'tu-1' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'pwd' }, tool_use_id: 'tu-2' },
      { type: 'CodingAgentToolResult', name: 'Bash', result: '/home/user', tool_use_id: 'tu-2' },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'a.txt\nb.txt', tool_use_id: 'tu-1' },
      { type: 'CodingAgentIdled' },
    ]);

    const exchanges = getExchanges(map, id);
    const stepEvents = exchangeResponseEvents(exchanges[0]).filter(e => e.type === 'step') as Array<{ description: string; tool_use_id?: string; result?: string }>;
    const tu1 = stepEvents.find(s => s.tool_use_id === 'tu-1');
    const tu2 = stepEvents.find(s => s.tool_use_id === 'tu-2');
    expect(tu1?.result).toBe('a.txt\nb.txt');
    expect(tu2?.result).toBe('/home/user');
  });

  it('ContextAssembled attaches assembled-prompt context to subsequent steps in the exchange', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Search my notes' },
      {
        type: 'ContextAssembled',
        sections: [
          { name: 'System Instructions', content: 'You are a helpful assistant.', budget_delta_chars: 28 },
          { name: 'User Message', content: 'Search my notes', budget_delta_chars: 15 },
        ],
        tools: ['search_memory', 'read_file'],
        model: 'claude-opus-4-7',
        total_chars: 43,
      },
      { type: 'MemorySearched', results: 3, queries: ['notes'] },
      { type: 'ResponseGenerated' },
    ] as Array<ThreadEvent & { created?: string }>);

    const exchanges = getExchanges(map, id);
    const stepEvents = exchangeResponseEvents(exchanges[0]).filter(e => e.type === 'step') as Array<{ description: string; context?: { sections: Array<{ name: string }>; tools: string[]; model: string; total_chars: number } }>;
    expect(stepEvents).toHaveLength(1);
    expect(stepEvents[0].context).toBeDefined();
    expect(stepEvents[0].context?.sections.map(s => s.name)).toEqual(['System Instructions', 'User Message']);
    expect(stepEvents[0].context?.tools).toEqual(['search_memory', 'read_file']);
    expect(stepEvents[0].context?.model).toBe('claude-opus-4-7');
    expect(stepEvents[0].context?.total_chars).toBe(43);
  });

  it('exchange with only TextStreamed and no tools shows response without steps', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Simple question' },
      { type: 'TextStreamed', text: 'Simple answer.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeSteps(exchanges[0])).toHaveLength(0);
    expect(exchangeResponseText(exchanges[0])).toBe('Simple answer.');
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    expect(hidesEarlierProse(exchangeResponseEvents(exchanges[0]))).toBe(false);
  });

  it('canceled exchange shows Canceled label', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Cancel me' },
      { type: 'ToolCalled', name: 'slow', args: {} },
      { type: 'ResponseCanceled' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('canceled');
    expect(getLabel(exchanges[0])).toBe('Canceled');
  });

  it('non-last exchange with CC idle forced to done', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'First', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:02Z' },
      { type: 'MessageReceived', text: 'Second', created: '2026-01-01T00:01:00Z' },
      { type: 'TextStreamed', text: 'Response', created: '2026-01-01T00:01:01Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:01:02Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    expect(getLabel(exchanges[0], '', false)).toBe('Done');
    expect(exchangeStatus(exchanges[1], '', true)).toBe('done');
  });

  it('CC section break only when CC has actual content', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Start CC' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'TextStreamed', text: 'Regular response before CC.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    const events = exchangeResponseEvents(exchanges[0]);
    const sectionBreaks = events.filter(e => e.type === 'section_break');
    expect(sectionBreaks).toHaveLength(0);
  });

  it('response text with no completion event and no buffer → aborted (old data)', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Old question', created: '2025-01-01T00:00:00Z' },
      { type: 'TextStreamed', text: 'Old answer', created: '2025-01-01T00:00:01Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('streaming');
    expect(exchangeResponseText(exchanges[0])).toBe('Old answer');
    expect(getLabel(exchanges[0])).toBe('Requesting');
  });

  it('empty exchange (just MessageReceived, nothing else) shows pending', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Just sent' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeSteps(exchanges[0])).toHaveLength(0);
    expect(exchangeResponseText(exchanges[0])).toBe('');
  });

  it('thread with ResponseGenerated text shows it', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Hello' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
  });
});

describe('Flow: post-restart resume reminder', () => {
  it('question answered after restart → no boundary, no reminder, reply under the card', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'ask test user q', channel: 'chat', event_id: 'mr-1' },
      { type: 'ToolCalled', name: 'ask_user_question', args: {}, request_event_id: 'mr-1', event_id: 'call-1' },
      {
        type: 'UserQuestionAsked',
        tool_use_id: 'toolu-1#q0',
        question: 'Which test option would you like to choose?',
        options: [
          { id: 'opt-0', label: 'Option A' },
          { id: 'opt-1', label: 'Option B' },
        ],
      },
      { type: 'UserQuestionAnswered', tool_use_id: 'toolu-1#q0', answer: { kind: 'Selected', option_id: 'opt-0' } },
      {
        type: 'ToolResult',
        name: 'ask_user_question',
        result: '{"Which test option would you like to choose?":"Option A"}',
        tool_called_event_id: 'call-1',
        request_event_id: 'mr-1',
      },
      { type: 'TextStreamed', text: 'You selected **Option A**.', request_event_id: 'mr-1' },
      { type: 'ResponseGenerated', text: 'You selected **Option A**.', request_event_id: 'mr-1' },
    ] as Array<ThreadEvent & { created?: string; event_id?: string }>);

    const exchanges = getExchanges(map, id);
    expect(exchanges.map(e => e.userEvent.type)).not.toContain('ContinuationStarted');
    expect(exchanges.every(e => resumeEngineNote(e) === null)).toBe(true);

    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(divider).toBeDefined();
    expect(exchangeResponseText(divider)).toContain('Option A');
    expect(exchangeStatus(divider, '', true)).toBe('done');
  });

  it('genuinely interrupted response revived by Continue → boundary AND reminder', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'ping me', channel: 'chat', event_id: 'mr-2' },
      { type: 'ToolCalled', name: 'send_notification', args: {}, request_event_id: 'mr-2' },
      { type: 'ToolResult', name: 'send_notification', result: 'ok', request_event_id: 'mr-2' },
      { type: 'ResponseAborted', request_event_id: 'mr-2', cause: 'recovery_after_restart' },
      { type: 'ContinuationStarted', actor: { kind: 'device', device_id: 'd-1' }, event_id: 'cs-1' },
      {
        type: 'PromptInjected',
        mode: 'engine',
        text: '[Engine note — this is a rerun]\n'
          + 'The interrupted run performed the following actions before the abort:\n'
          + '- send_notification(Ping) → ok',
        request_event_id: 'cs-1',
      },
      { type: 'TextStreamed', text: 'Already pinged you.', request_event_id: 'cs-1' },
      { type: 'ResponseGenerated', text: 'Already pinged you.', request_event_id: 'cs-1' },
    ] as Array<ThreadEvent & { created?: string; event_id?: string }>);

    const exchanges = getExchanges(map, id);
    const resume = exchanges.find(e => e.userEvent.type === 'ContinuationStarted')!;
    expect(resume).toBeDefined();
    expect(exchangeUserMessage(resume)).toBe('Continued the response');
    const note = resumeEngineNote(resume);
    expect(note).not.toBeNull();
    expect(note!.toolCount).toBe(1);
    expect(note!.text).toContain('send_notification');
  });
});

describe('Flow: an engine-restart resume reads in clock order', () => {
  function resumedAfterRestart() {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'keep going', channel: 'chat', event_id: 'mr-1', created: '2026-01-01T07:51:29Z' },
      { type: 'ToolCalled', name: 'read_file', args: {}, request_event_id: 'mr-1', event_id: 'call-1', created: '2026-01-01T07:55:11Z' },
      { type: 'ToolResult', name: 'read_file', result: 'ok', tool_called_event_id: 'call-1', request_event_id: 'mr-1', created: '2026-01-01T07:55:12Z' },
      { type: 'ResponseAborted', request_event_id: 'mr-1', cause: 'recovery_after_restart', created: '2026-01-01T08:00:09Z' },
      { type: 'PromptInjected', text: 'A subscribed event arrived.', mode: 'agent', event_id: 'wake-1', created: '2026-01-01T08:00:38Z' },
      { type: 'ContinuationStarted', event_id: 'cs-1', created: '2026-01-01T08:00:39Z' },
      { type: 'PromptInjected', text: '[Engine note] 50 prior tool calls', mode: 'engine', request_event_id: 'cs-1', created: '2026-01-01T08:00:39.400Z' },
      { type: 'TextStreamed', text: 'Picking it back up.', request_event_id: 'wake-1', created: '2026-01-01T08:00:55Z' },
      { type: 'ToolCalled', name: 'read_file', args: {}, request_event_id: 'wake-1', event_id: 'call-2', created: '2026-01-01T08:16:36Z' },
      { type: 'ToolResult', name: 'read_file', result: 'ok', tool_called_event_id: 'call-2', request_event_id: 'wake-1', created: '2026-01-01T08:16:37Z' },
      { type: 'TextStreamed', text: 'Done reading.', request_event_id: 'wake-1', created: '2026-01-01T08:19:00Z' },
    ] as Array<ThreadEvent & { created?: string; event_id?: string }>);
    return getExchanges(map, id);
  }

  it('files the resumed work under the resume boundary, not above it', () => {
    const exchanges = resumedAfterRestart();
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived', 'ResponseAborted', 'PromptInjected', 'ContinuationStarted',
    ]);
    expect(exchanges[2].steps).toHaveLength(0);
    expect(exchanges[3].steps.map(s => s.event.type)).toEqual([
      'PromptInjected', 'TextStreamed', 'ToolCalled', 'ToolResult', 'TextStreamed',
    ]);
  });

  it('leaves no card spinning "Requesting" under the running work', () => {
    const exchanges = resumedAfterRestart();
    const last = exchanges.length - 1;
    expect(exchanges.map((ex, i) => getLabel(ex, '', i === last))).toEqual([
      'Aborted', 'Done', 'Done', 'Working',
    ]);
  });

  it('renders every row in clock order', () => {
    expect(renderOrderViolations(resumedAfterRestart())).toEqual([]);
  });
});
