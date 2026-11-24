import { describe, it, expect } from 'vitest';
import { buildCCThread, makeThreadState } from './thread-events-helpers';
import { describeCCTool, exchangeResponseEvents, exchangeSteps, fullCommandForCCTool, fullCommandForEngineTool, groupIntoExchanges, handleEvent, shellScriptBody, type StoredEvent, type ThreadEvent } from '../thread-events';
import type { StepOutcome } from '../types';

describe('PromptInjected in groupIntoExchanges', () => {
  it('PromptInjected starts a new exchange', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'do X' }],
      [2, { type: 'ToolCalled', name: 'web_search', args: {} }],
      [3, { type: 'ToolResult', name: 'web_search', result: 'ok' }],
      [4, { type: 'PromptInjected', text: 'actually do Y' }],
      [5, { type: 'TextStreamed', text: 'doing Y now' }],
      [6, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].userEvent).toEqual({ type: 'MessageReceived', text: 'do X' });
    expect(exchanges[0].steps).toHaveLength(2);
    expect(exchanges[1].userEvent).toEqual({ type: 'PromptInjected', text: 'actually do Y' });
    expect(exchanges[1].steps).toHaveLength(2);
  });

  it('multiple injections create multiple exchanges', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'start' }],
      [2, { type: 'ToolCalled', name: 'search', args: {} }],
      [3, { type: 'PromptInjected', text: 'correction 1' }],
      [4, { type: 'ToolCalled', name: 'read', args: {} }],
      [5, { type: 'PromptInjected', text: 'correction 2' }],
      [6, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[1].userEvent.type).toBe('PromptInjected');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('correction 1');
    expect(exchanges[2].userEvent.type).toBe('PromptInjected');
    expect((exchanges[2].userEvent as { text: string }).text).toBe('correction 2');
  });

  it('PromptInjected with injected_message_id absorbs into matching MessageReceived exchange', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'first', _eventId: 'msg-1' }],
      [2, { type: 'ResponseGenerated', text: 'first response' }],
      [3, { type: 'MessageReceived', text: 'follow-up', _eventId: 'msg-2' }],
      [4, { type: 'PromptInjected', text: 'follow-up', injected_message_id: 'msg-2' }],
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
      'PromptInjected', 'TextStreamed', 'ResponseGenerated',
    ]);
  });

  it('PromptInjected without injected_message_id still starts its own exchange (legacy)', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'start', _eventId: 'msg-1' }],
      [2, { type: 'PromptInjected', text: 'inject' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('PromptInjected');
  });

  it('PromptInjected with injected_message_id and no matching MessageReceived falls back to its own exchange', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'start', _eventId: 'msg-1' }],
      [2, { type: 'PromptInjected', text: 'orphan', injected_message_id: 'missing' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('PromptInjected');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('orphan');
  });

  it('queued mid-flight message ingested AFTER a question renders its reply below the question card', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'original', _eventId: 'req-orig' }],
      [2, { type: 'TextStreamed', text: 'working on original', request_event_id: 'req-orig' } as StoredEvent],
      [3, { type: 'MessageReceived', text: 'cancel button broke', _eventId: 'msg-cancel' }],
      [4, { type: 'UserQuestionAsked', question: 'pick one', tool_use_id: 'tuid-q', _eventId: 'uqa' } as StoredEvent],
      [5, { type: 'UserQuestionAnswered', tool_use_id: 'tuid-q' } as StoredEvent],
      [6, { type: 'PromptInjected', text: 'cancel button broke', injected_message_id: 'msg-cancel', request_event_id: 'req-orig' } as StoredEvent],
      [7, { type: 'TextStreamed', text: 'analyzing the cancel bug', request_event_id: 'req-orig' } as StoredEvent],
      [8, { type: 'ResponseGenerated', text: 'done', request_event_id: 'req-orig' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const qIdx = exchanges.findIndex(e => e.userEvent.type === 'UserQuestionAsked');
    const cancelIdx = exchanges.findIndex(e =>
      e.userEvent.type === 'MessageReceived' &&
      (e.userEvent as { text?: string }).text === 'cancel button broke');
    expect(qIdx).toBeGreaterThanOrEqual(0);
    expect(cancelIdx).toBeGreaterThan(qIdx);
    expect(exchanges[cancelIdx].steps.map(s => s.event.type)).toContain('TextStreamed');
  });
});

describe('step completion — no eternal spinners', () => {
  it('parallel CC tool calls all resolve when results arrive', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'fix it' } as ThreadEvent, '2026-04-04T10:00:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-04T10:00:01Z');
    handleEvent(map, 'thread-1', 3, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'a.rs' } } as ThreadEvent, '2026-04-04T10:00:02Z');
    handleEvent(map, 'thread-1', 4, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'b.rs' } } as ThreadEvent, '2026-04-04T10:00:02Z');
    handleEvent(map, 'thread-1', 5, { type: 'CodingAgentToolResult', name: '', result: 'ok' } as ThreadEvent, '2026-04-04T10:00:03Z');
    handleEvent(map, 'thread-1', 6, { type: 'CodingAgentTextStreamed', text: 'Done!' } as ThreadEvent, '2026-04-04T10:00:04Z');
    handleEvent(map, 'thread-1', 7, { type: 'CodingAgentIdled' } as ThreadEvent, '2026-04-04T10:00:05Z');

    const exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    expect(exchanges).toHaveLength(1);

    const events = exchangeResponseEvents(exchanges[0]);
    const steps = events.filter(e => e.type === 'step');

    for (const step of steps) {
      expect((step as { outcome: StepOutcome }).outcome).toBe('success');
    }
  });

  it('missing ToolResult on completed exchange shows checkmark, not spinner', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'fix it' } as ThreadEvent, '2026-04-04T10:00:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-04T10:00:01Z');
    handleEvent(map, 'thread-1', 3, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, '2026-04-04T10:00:02Z');
    handleEvent(map, 'thread-1', 4, { type: 'CodingAgentIdled' } as ThreadEvent, '2026-04-04T10:00:03Z');

    const exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    const events = exchangeResponseEvents(exchanges[0]);
    const steps = events.filter(e => e.type === 'step');

    expect(steps).toHaveLength(1);
    expect((steps[0] as { outcome: StepOutcome }).outcome).toBe('success');
  });

  it('does NOT force-resolve spinners when CC resumed after idle', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'fix it' } as ThreadEvent, '2026-04-04T10:00:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-04T10:00:01Z');
    handleEvent(map, 'thread-1', 3, { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, '2026-04-04T10:00:02Z');
    handleEvent(map, 'thread-1', 4, { type: 'CodingAgentToolResult', name: '', result: 'ok' } as ThreadEvent, '2026-04-04T10:00:03Z');
    handleEvent(map, 'thread-1', 5, { type: 'CodingAgentIdled' } as ThreadEvent, '2026-04-04T10:00:04Z');
    handleEvent(map, 'thread-1', 6, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, '2026-04-04T10:00:05Z');

    const exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    const events = exchangeResponseEvents(exchanges[0]);
    const steps = events.filter(e => e.type === 'step');

    const lastStep = steps[steps.length - 1] as { outcome: StepOutcome };
    expect(lastStep.outcome).toBe('pending');
  });

  it('three parallel subagents resolve individually as results arrive (live streaming)', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'fix it' } as ThreadEvent, '2026-04-04T10:00:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-04T10:00:01Z');
    handleEvent(map, 'thread-1', 3, { type: 'CodingAgentToolCalled', name: 'Agent', args: { prompt: 'task 1' } } as ThreadEvent, '2026-04-04T10:00:02Z');
    handleEvent(map, 'thread-1', 4, { type: 'CodingAgentToolCalled', name: 'Agent', args: { prompt: 'task 2' } } as ThreadEvent, '2026-04-04T10:00:02Z');
    handleEvent(map, 'thread-1', 5, { type: 'CodingAgentToolCalled', name: 'Agent', args: { prompt: 'task 3' } } as ThreadEvent, '2026-04-04T10:00:02Z');

    handleEvent(map, 'thread-1', 6, { type: 'CodingAgentToolResult', name: '', result: 'done 1' } as ThreadEvent, '2026-04-04T10:00:05Z');

    let exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    let events = exchangeResponseEvents(exchanges[0]);
    let steps = events.filter(e => e.type === 'step') as { outcome: StepOutcome }[];
    let resolved = steps.filter(s => s.outcome === 'success').length;
    let pending = steps.filter(s => s.outcome === 'pending').length;
    expect(resolved).toBe(1);
    expect(pending).toBe(2);

    handleEvent(map, 'thread-1', 7, { type: 'CodingAgentToolResult', name: '', result: 'done 2' } as ThreadEvent, '2026-04-04T10:00:06Z');
    exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    events = exchangeResponseEvents(exchanges[0]);
    steps = events.filter(e => e.type === 'step') as { outcome: StepOutcome }[];
    resolved = steps.filter(s => s.outcome === 'success').length;
    pending = steps.filter(s => s.outcome === 'pending').length;
    expect(resolved).toBe(2);
    expect(pending).toBe(1);

    handleEvent(map, 'thread-1', 8, { type: 'CodingAgentToolResult', name: '', result: 'done 3' } as ThreadEvent, '2026-04-04T10:00:07Z');
    exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    events = exchangeResponseEvents(exchanges[0]);
    steps = events.filter(e => e.type === 'step') as { outcome: StepOutcome }[];
    resolved = steps.filter(s => s.outcome === 'success').length;
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
    expect(respSteps).toHaveLength(4);
    for (const step of respSteps.slice(0, 3)) {
      expect((step as { outcome: StepOutcome }).outcome).toBe('success');
    }
    expect(respSteps[3]).toMatchObject({ description: 'Thinking', outcome: 'pending' });

    const steps = exchangeSteps(exchanges[0]);
    expect(steps).toHaveLength(4);
    for (const step of steps.slice(0, 3)) {
      expect(step.outcome).toBe('success');
    }
    expect(steps[3]).toMatchObject({ description: 'Thinking', outcome: 'pending' });
  });

  it('parallel CC reads with same description: result pairs by tool_use_id, not visual order', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'audit skills', created: '2026-04-04T10:00:00Z' } as ThreadEvent],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-04-04T10:00:01Z' } as ThreadEvent],
      [3, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: '/skills/grill-me/SKILL.md' }, tool_use_id: 'tu-A', created: '2026-04-04T10:00:02Z' } as ThreadEvent],
      [4, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: '/skills/superpowers/SKILL.md' }, tool_use_id: 'tu-B', created: '2026-04-04T10:00:02Z' } as ThreadEvent],
      [5, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', tool_use_id: 'tu-A', created: '2026-04-04T10:00:03Z' } as ThreadEvent],
    ]);

    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);

    const respSteps = exchangeResponseEvents(exchanges[0]).filter(e => e.type === 'step') as { tool_use_id?: string; outcome: StepOutcome }[];
    expect(respSteps).toHaveLength(2);
    const stepA = respSteps.find(s => s.tool_use_id === 'tu-A');
    const stepB = respSteps.find(s => s.tool_use_id === 'tu-B');
    expect(stepA?.outcome).toBe('success');
    expect(stepB?.outcome).toBe('pending');

    const steps = exchangeSteps(exchanges[0]) as { tool_use_id?: string; outcome: StepOutcome }[];
    expect(steps).toHaveLength(2);
    expect(steps.find(s => s.tool_use_id === 'tu-A')?.outcome).toBe('success');
    expect(steps.find(s => s.tool_use_id === 'tu-B')?.outcome).toBe('pending');
  });

  it('legacy CC events without tool_use_id fall back to backward-walk resolution', () => {
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
    const respSteps = exchangeResponseEvents(exchanges[0]).filter(e => e.type === 'step') as { description: string; outcome: StepOutcome }[];
    expect(respSteps).toHaveLength(3);
    for (const step of respSteps.slice(0, 2)) expect(step.outcome).toBe('success');
    expect(respSteps[2]).toMatchObject({ description: 'Thinking', outcome: 'pending' });
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
      expect((step as { outcome: StepOutcome }).outcome).toBe('success');
    }

    const steps = exchangeSteps(exchanges[0]);
    expect(steps).toHaveLength(2);
    for (const step of steps) {
      expect(step.outcome).toBe('success');
    }
  });

  it('a parallel result resolves the row of the call it names, in any order', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'search', created: '2026-04-04T10:00:00Z' } as ThreadEvent],
      [2, { type: 'ToolCalled', name: 'web_search', args: { query: 'a' }, _eventId: 'call-a', created: '2026-04-04T10:00:01Z' } as StoredEvent],
      [3, { type: 'ToolCalled', name: 'web_search', args: { query: 'b' }, _eventId: 'call-b', created: '2026-04-04T10:00:01Z' } as StoredEvent],
      [4, { type: 'ToolResult', name: 'web_search', result: 'res-a', tool_called_event_id: 'call-a', _eventId: 'result-a', created: '2026-04-04T10:00:02Z' } as StoredEvent],
    ]);

    const midRun = exchangeResponseEvents(groupIntoExchanges(events)[0])
      .filter(e => e.type === 'step') as { call_event_id?: string; outcome: StepOutcome; result?: string }[];
    expect(midRun.map(s => [s.call_event_id, s.outcome, s.result])).toEqual([
      ['call-a', 'success', 'res-a'],
      ['call-b', 'pending', undefined],
    ]);
    const midSteps = exchangeSteps(groupIntoExchanges(events)[0]);
    expect(midSteps.map(s => [s.call_event_id, s.outcome])).toEqual([
      ['call-a', 'success'],
      ['call-b', 'pending'],
    ]);

    events.set(5, { type: 'ToolResult', name: 'web_search', result: 'res-b', tool_called_event_id: 'call-b', _eventId: 'result-b', created: '2026-04-04T10:00:03Z' } as StoredEvent);
    const settled = exchangeResponseEvents(groupIntoExchanges(events)[0])
      .filter(e => e.type === 'step') as { call_event_id?: string; result?: string; result_event_id?: string }[];
    expect(settled.map(s => [s.call_event_id, s.result, s.result_event_id])).toEqual([
      ['call-a', 'res-a', 'result-a'],
      ['call-b', 'res-b', 'result-b'],
    ]);
  });

  it('a legacy result with no call id still resolves the newest pending row', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'search', created: '2026-04-04T10:00:00Z' } as ThreadEvent],
      [2, { type: 'ToolCalled', name: 'web_search', args: { query: 'a' }, _eventId: 'call-a', created: '2026-04-04T10:00:01Z' } as StoredEvent],
      [3, { type: 'ToolCalled', name: 'web_search', args: { query: 'b' }, _eventId: 'call-b', created: '2026-04-04T10:00:01Z' } as StoredEvent],
      [4, { type: 'ToolResult', name: 'web_search', result: 'res', created: '2026-04-04T10:00:02Z' } as ThreadEvent],
    ]);
    const steps = exchangeResponseEvents(groupIntoExchanges(events)[0])
      .filter(e => e.type === 'step') as { call_event_id?: string; outcome: StepOutcome }[];
    expect(steps.map(s => [s.call_event_id, s.outcome])).toEqual([
      ['call-a', 'pending'],
      ['call-b', 'success'],
    ]);
  });

  it('a failed result settles its row to error in both projections', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'run it', created: '2026-04-04T10:00:00Z' } as ThreadEvent],
      [2, { type: 'ToolCalled', name: 'bash', args: { command: 'false' }, _eventId: 'call-a', created: '2026-04-04T10:00:01Z' } as StoredEvent],
      [3, { type: 'ToolCalled', name: 'bash', args: { command: 'true' }, _eventId: 'call-b', created: '2026-04-04T10:00:01Z' } as StoredEvent],
      [4, { type: 'ToolCalled', name: 'bash', args: { command: 'ls' }, _eventId: 'call-c', created: '2026-04-04T10:00:01Z' } as StoredEvent],
      [5, { type: 'ToolResult', name: 'bash', result: 'Error: exit 1', success: false, tool_called_event_id: 'call-a', created: '2026-04-04T10:00:02Z' } as StoredEvent],
      [6, { type: 'ToolResult', name: 'bash', result: 'ok', success: true, tool_called_event_id: 'call-b', created: '2026-04-04T10:00:02Z' } as StoredEvent],
      [7, { type: 'ToolResult', name: 'bash', result: 'ok', tool_called_event_id: 'call-c', created: '2026-04-04T10:00:02Z' } as StoredEvent],
      [8, { type: 'ResponseGenerated', created: '2026-04-04T10:00:03Z' } as ThreadEvent],
    ]);
    const expected = [
      ['call-a', 'error'],
      ['call-b', 'success'],
      ['call-c', 'success'],
    ];
    const exchange = groupIntoExchanges(events)[0];
    const respSteps = exchangeResponseEvents(exchange)
      .filter(e => e.type === 'step') as { call_event_id?: string; outcome: StepOutcome; result?: string }[];
    expect(respSteps.map(s => [s.call_event_id, s.outcome])).toEqual(expected);
    expect(respSteps[0].result).toBe('Error: exit 1');
    expect(exchangeSteps(exchange).map(s => [s.call_event_id, s.outcome])).toEqual(expected);
  });

  it('exchangeSteps also resolves pending steps on completed exchange', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'fix it' } as ThreadEvent, '2026-04-04T10:00:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, '2026-04-04T10:00:01Z');
    handleEvent(map, 'thread-1', 3, { type: 'ResponseGenerated' } as ThreadEvent, '2026-04-04T10:00:02Z');

    const exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    const steps = exchangeSteps(exchanges[0]);

    expect(steps).toHaveLength(1);
    expect(steps[0].outcome).toBe('success');
  });
});


function killedMidCall(terminal: Record<string, unknown>): Map<number, ThreadEvent> {
  return new Map<number, ThreadEvent>([
    [1, { type: 'MessageReceived', text: 'run the linter', created: '2026-08-03T07:44:30Z' } as ThreadEvent],
    [2, { type: 'SessionStarted', session_id: 's1', created: '2026-08-03T07:44:31Z' } as ThreadEvent],
    [3, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'scripts/lib/webkit_shard.sh' }, tool_use_id: 'tu-done', created: '2026-08-03T07:44:35Z' } as ThreadEvent],
    [4, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', tool_use_id: 'tu-done', created: '2026-08-03T07:44:36Z' } as ThreadEvent],
    [5, { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'shellcheck scripts/lib/webkit_shard.sh' }, tool_use_id: 'tu-killed', created: '2026-08-03T07:44:41Z' } as ThreadEvent],
    [6, { ...terminal, created: '2026-08-03T07:44:44Z' } as unknown as ThreadEvent],
  ]);
}

function stepOutcomes(events: Map<number, ThreadEvent>) {
  const exchanges = groupIntoExchanges(events);
  const inline = exchangeResponseEvents(exchanges[0])
    .filter(e => e.type === 'step') as { tool_use_id?: string; outcome: string }[];
  const list = exchangeSteps(exchanges[0]) as unknown as { tool_use_id?: string; outcome: string }[];
  return { inline, list };
}

describe('step outcome when a tool call never returned', () => {
  const UNCLEAN_TERMINALS = [
    { name: 'ResponseFailed', event: { type: 'ResponseFailed', error: 'Event stream error' } },
    { name: 'ResponseAborted', event: { type: 'ResponseAborted' } },
    { name: 'ResponseCanceled', event: { type: 'ResponseCanceled' } },
  ];

  for (const { name, event } of UNCLEAN_TERMINALS) {
    it(`${name}: the in-flight step reads unfinished, never pending and never a checkmark`, () => {
      const { inline, list } = stepOutcomes(killedMidCall(event));

      for (const steps of [inline, list]) {
        expect(steps).toHaveLength(2);
        const killed = steps.find(s => s.tool_use_id === 'tu-killed');
        expect(killed?.outcome).toBe('unfinished');
        expect(steps.find(s => s.tool_use_id === 'tu-done')?.outcome).toBe('success');
      }
    });
  }

  const CLEAN_TERMINALS = [
    { name: 'ResponseGenerated', event: { type: 'ResponseGenerated', text: 'linted' } },
    { name: 'CodingAgentIdled', event: { type: 'CodingAgentIdled', has_changes: false } },
  ];

  for (const { name, event } of CLEAN_TERMINALS) {
    it(`${name}: the same shape resolves to a checkmark, the turn finished`, () => {
      const { inline, list } = stepOutcomes(killedMidCall(event));

      for (const steps of [inline, list]) {
        expect(steps).toHaveLength(2);
        expect(steps.find(s => s.tool_use_id === 'tu-killed')?.outcome).toBe('success');
        expect(steps.find(s => s.tool_use_id === 'tu-done')?.outcome).toBe('success');
      }
    });
  }

  it('a later same-request terminal wins: the recovered engine-restart turn keeps its checkmarks', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'run the linter', _eventId: 'req-1', created: '2026-08-03T07:44:30Z' } as ThreadEvent],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-08-03T07:44:31Z' } as ThreadEvent],
      [3, { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'shellcheck x.sh' }, tool_use_id: 'tu-1', request_event_id: 'req-1', created: '2026-08-03T07:44:41Z' } as ThreadEvent],
      [4, { type: 'ResponseAborted', request_event_id: 'req-1', created: '2026-08-03T07:44:44Z' } as ThreadEvent],
      [5, { type: 'ResponseGenerated', text: 'done', request_event_id: 'req-1', created: '2026-08-03T07:45:10Z' } as ThreadEvent],
    ]);
    const { inline, list } = stepOutcomes(events);

    for (const steps of [inline, list]) {
      expect(steps.find(s => s.tool_use_id === 'tu-1')?.outcome).toBe('success');
    }
  });
});

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

  it('ContextTokensMeasured overwrites the previous Thinking step context_tokens in both projections', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'hi' } as ThreadEvent, '2026-04-09T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed', text: 'estimate', context_tokens: 1_374_000, context_messages: 1 } as ThreadEvent, '2026-04-09T10:00:01Z');
    handleEvent(map, 't', 3, { type: 'ContextTokensMeasured', input_tokens: 23_500 } as ThreadEvent, '2026-04-09T10:00:02Z');
    handleEvent(map, 't', 4, { type: 'ResponseGenerated' } as ThreadEvent, '2026-04-09T10:00:03Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);

    const steps = exchangeSteps(exchanges[0]) as { description: string; context_tokens?: number }[];
    const stepThinking = steps.find((s) => s.description === 'Thinking');
    expect(stepThinking?.context_tokens).toBe(23_500);

    const inlineEvents = exchangeResponseEvents(exchanges[0]);
    const inlineThinking = inlineEvents.find(
      (e): e is Extract<typeof e, { type: 'step' }> =>
        e.type === 'step' && e.description === 'Thinking',
    );
    expect(inlineThinking?.context_tokens).toBe(23_500);
  });

  it('Thinking stays pending while the chat-agent LLM call is still running', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, '2026-05-24T19:00:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed', text: 'Context: 100 tokens, 1 messages' } as ThreadEvent, '2026-05-24T19:00:01Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const steps = exchangeSteps(exchanges[0],  true);
    expect(steps).toHaveLength(1);
    expect(steps[0].description).toBe('Thinking');
    expect(steps[0].outcome).toBe('pending');

    const respEvents = exchangeResponseEvents(exchanges[0]);
    const respSteps = respEvents.filter((e): e is Extract<typeof e, { type: 'step' }> => e.type === 'step');
    expect(respSteps).toHaveLength(1);
    expect(respSteps[0].outcome).toBe('pending');
  });

  it('ToolCalled NAMES the Thinking row rather than opening a second one', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, '2026-05-24T19:00:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed', text: 'Context: 100 tokens, 1 messages' } as ThreadEvent, '2026-05-24T19:00:01Z');
    handleEvent(map, 't', 3, { type: 'ToolCalled', name: 'run_bash', args: { command: 'ls' } } as ThreadEvent, '2026-05-24T19:00:02Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const steps = exchangeSteps(exchanges[0],  true);
    expect(steps).toHaveLength(1);
    expect(steps[0].description).toBe('Run ls');
    expect(steps[0].outcome).toBe('pending');

    const respSteps = exchangeResponseEvents(exchanges[0]).filter(
      (e): e is Extract<typeof e, { type: 'step' }> => e.type === 'step',
    );
    expect(respSteps).toHaveLength(1);
    expect(respSteps[0].description).toBe('Run ls');
    expect(respSteps[0].tool_name).toBe('run_bash');
    expect(respSteps[0].outcome).toBe('pending');
  });

  it('only the FIRST of a pass\'s tool calls takes the row; parallel calls get their own', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, '2026-05-24T19:00:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed', text: 'Context: 100 tokens, 1 messages' } as ThreadEvent, '2026-05-24T19:00:01Z');
    handleEvent(map, 't', 3, { type: 'ToolCalled', name: 'run_bash', args: { command: 'ls' } } as ThreadEvent, '2026-05-24T19:00:02Z');
    handleEvent(map, 't', 4, { type: 'ToolCalled', name: 'run_bash', args: { command: 'pwd' } } as ThreadEvent, '2026-05-24T19:00:03Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const steps = exchangeSteps(exchanges[0],  true);
    expect(steps.map(s => s.description)).toEqual(['Run ls', 'Run pwd']);
    expect(steps.every(s => s.outcome === 'pending')).toBe(true);
  });

  it('a blank text chunk before a coding-agent tool call does not cost the fold its row', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'go', created: '2026-08-06T09:14:00Z' } as ThreadEvent],
      [2, { type: 'CodingAgentPromptSent', text: 'go', created: '2026-08-06T09:14:00Z' } as ThreadEvent],
      [3, { type: 'CodingAgentTextStreamed', text: ' ', created: '2026-08-06T09:14:01Z' } as ThreadEvent],
      [4, { type: 'CodingAgentToolCalled', name: 'Grep', args: { pattern: 'Thinking' }, tool_use_id: 'tu-A', created: '2026-08-06T09:14:01Z' } as ThreadEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const respSteps = exchangeResponseEvents(exchanges[0]).filter(
      (e): e is Extract<typeof e, { type: 'step' }> => e.type === 'step',
    );
    expect(respSteps).toHaveLength(1);
    expect(respSteps[0].description).not.toBe('Thinking');
    expect(respSteps[0].tool_use_id).toBe('tu-A');

    const steps = exchangeSteps(exchanges[0]);
    expect(steps).toHaveLength(1);
    expect(steps[0].description).not.toBe('Thinking');
  });

  it('prose before a tool call does not cost the pass its row, and the row lands below the prose', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'go', created: '2026-08-06T09:14:00Z' } as ThreadEvent],
      [2, { type: 'CodingAgentPromptSent', text: 'go', created: '2026-08-06T09:14:00Z' } as ThreadEvent],
      [3, { type: 'CodingAgentTextStreamed', text: 'Let me look at the projection.', created: '2026-08-06T09:14:01Z' } as ThreadEvent],
      [4, { type: 'CodingAgentToolCalled', name: 'Grep', args: { pattern: 'Thinking' }, tool_use_id: 'tu-A', created: '2026-08-06T09:14:01Z' } as ThreadEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const respEvents = exchangeResponseEvents(exchanges[0]);
    expect(respEvents.map(e => e.type)).toEqual(['text', 'step']);
    const respSteps = respEvents.filter(
      (e): e is Extract<typeof e, { type: 'step' }> => e.type === 'step',
    );
    expect(respSteps.map(s => s.description)).toEqual(["Search 'Thinking'"]);
    expect(respSteps[0].outcome).toBe('pending');
    expect(respSteps[0].tool_use_id).toBe('tu-A');

    const steps = exchangeSteps(exchanges[0]);
    expect(steps.map(s => s.description)).toEqual(["Search 'Thinking'"]);
  });

  it('a narrated chat pass keeps its context counter on the row the call took over', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, '2026-08-12T09:00:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed', text: 'Context: 100 tokens, 1 messages' } as ThreadEvent, '2026-08-12T09:00:01Z');
    handleEvent(map, 't', 3, { type: 'TextStreamed', text: 'Let me look at where it fires.' } as ThreadEvent, '2026-08-12T09:00:02Z');
    handleEvent(map, 't', 4, {
      type: 'ContextCaptured',
      producer: 'main_llm',
      model: 'claude-opus-5[1m]',
      context_window: 1_000_000,
      sections: [],
      tools: [],
      estimated_total_tokens: 69_000,
    } as unknown as ThreadEvent, '2026-08-12T09:00:03Z');
    handleEvent(map, 't', 5, { type: 'ToolCalled', name: 'run_bash', args: { command: 'ls' } } as ThreadEvent, '2026-08-12T09:00:04Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const respEvents = exchangeResponseEvents(exchanges[0]);
    expect(respEvents.map(e => e.type)).toEqual(['text', 'step']);
    const step = respEvents[1] as Extract<(typeof respEvents)[number], { type: 'step' }>;
    expect(step.description).toBe('Run ls');
    expect(step.outcome).toBe('pending');
    expect(step.contextCapture?.estimated_total_tokens).toBe(69_000);
  });

  it('an auxiliary capture leaves the turn its context chip', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, '2026-08-12T09:00:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed', text: 'thinking' } as ThreadEvent, '2026-08-12T09:00:01Z');
    handleEvent(map, 't', 3, {
      type: 'ContextCaptured',
      producer: 'main_llm',
      model: 'claude-opus-5[1m]',
      context_window: 1_000_000,
      sections: [],
      tools: [],
      estimated_total_tokens: 69_000,
    } as unknown as ThreadEvent, '2026-08-12T09:00:02Z');
    for (const [seq, purpose] of [[4, 'memory'], [5, 'title']] as const) {
      handleEvent(map, 't', seq, {
        type: 'ContextCaptured',
        producer: 'auxiliary',
        purpose,
        model: 'gemini-3.8-flash',
        context_window: 0,
        sections: [],
        tools: [],
        estimated_total_tokens: 300,
      } as unknown as ThreadEvent, '2026-08-12T09:00:03Z');
    }

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const steps = exchangeSteps(exchanges[0]);
    expect(steps).toHaveLength(1);
    expect(steps[0].contextCapture?.estimated_total_tokens).toBe(69_000);
    expect(steps[0].contextCapture?.model).toBe('claude-opus-5[1m]');
  });

  it('a capture with no purpose is a turn, so legacy rows still bind', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, '2026-08-12T09:00:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed', text: 'thinking' } as ThreadEvent, '2026-08-12T09:00:01Z');
    handleEvent(map, 't', 3, {
      type: 'ContextCaptured',
      producer: 'main_llm',
      model: 'claude-opus-5[1m]',
      context_window: 1_000_000,
      sections: [],
      tools: [],
      estimated_total_tokens: 42_000,
    } as unknown as ThreadEvent, '2026-08-12T09:00:02Z');

    const steps = exchangeSteps(groupIntoExchanges(map.get('t')!.events)[0]);
    expect(steps[0].contextCapture?.estimated_total_tokens).toBe(42_000);
  });

  it('a second narrated pass opens its own row rather than reclaiming the first', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, '2026-08-12T09:00:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed', text: 'Context: 100 tokens, 1 messages' } as ThreadEvent, '2026-08-12T09:00:01Z');
    handleEvent(map, 't', 3, { type: 'TextStreamed', text: 'First, the index.' } as ThreadEvent, '2026-08-12T09:00:02Z');
    handleEvent(map, 't', 4, { type: 'ToolCalled', name: 'run_bash', args: { command: 'ls' } } as ThreadEvent, '2026-08-12T09:00:03Z');
    handleEvent(map, 't', 5, { type: 'ToolResult', name: 'run_bash', result: 'ok' } as ThreadEvent, '2026-08-12T09:00:04Z');
    handleEvent(map, 't', 6, { type: 'ThoughtStreamed', text: 'Context: 200 tokens, 3 messages' } as ThreadEvent, '2026-08-12T09:00:05Z');
    handleEvent(map, 't', 7, { type: 'TextStreamed', text: 'Now the renderer.' } as ThreadEvent, '2026-08-12T09:00:06Z');
    handleEvent(map, 't', 8, { type: 'ToolCalled', name: 'run_bash', args: { command: 'pwd' } } as ThreadEvent, '2026-08-12T09:00:07Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const respEvents = exchangeResponseEvents(exchanges[0]);
    expect(respEvents.map(e => e.type)).toEqual(['text', 'step', 'text', 'step']);
    const steps = exchangeSteps(exchanges[0]);
    expect(steps.map(s => s.description)).toEqual(['Run ls', 'Run pwd']);
  });

  it('a tool call does not reclaim a marker left unfinished by a dead turn', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, '2026-08-12T09:00:00Z');
    handleEvent(map, 't', 2, { type: 'CodingAgentPromptSent', text: 'go' } as ThreadEvent, '2026-08-12T09:00:01Z');
    handleEvent(map, 't', 3, { type: 'ResponseAborted', reason: 'engine restart' } as ThreadEvent, '2026-08-12T09:00:02Z');

    const aborted = groupIntoExchanges(map.get('t')!.events)[0];
    const marker = exchangeSteps(aborted,  false)[0];
    expect(marker).toMatchObject({ description: 'Thinking', outcome: 'unfinished' });
  });

  it('a pass that answers in text keeps its Thinking row, which is where its counter lives', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, '2026-05-24T19:00:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed', text: 'Context: 100 tokens, 1 messages', context_tokens: 1_200 } as ThreadEvent, '2026-05-24T19:00:01Z');
    handleEvent(map, 't', 3, { type: 'TextStreamed', text: 'Here you go.' } as ThreadEvent, '2026-05-24T19:00:02Z');
    handleEvent(map, 't', 4, { type: 'ResponseGenerated', text: 'Here you go.' } as ThreadEvent, '2026-05-24T19:00:03Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const respSteps = exchangeResponseEvents(exchanges[0]).filter(
      (e): e is Extract<typeof e, { type: 'step' }> => e.type === 'step',
    );
    expect(respSteps).toHaveLength(1);
    expect(respSteps[0].description).toBe('Thinking');
    expect(respSteps[0].outcome).toBe('success');
    expect(respSteps[0].context_tokens).toBe(1_200);
  });

  it('Thinking resolves to ✓ when TextStreamed arrives', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, '2026-05-24T19:00:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed', text: 'Context: 100 tokens, 1 messages' } as ThreadEvent, '2026-05-24T19:00:01Z');
    handleEvent(map, 't', 3, { type: 'TextStreamed', text: 'hello' } as ThreadEvent, '2026-05-24T19:00:02Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const steps = exchangeSteps(exchanges[0],  true);
    expect(steps).toHaveLength(1);
    expect(steps[0].description).toBe('Thinking');
    expect(steps[0].outcome).toBe('success');

    const respSteps = exchangeResponseEvents(exchanges[0]).filter(
      (e): e is Extract<typeof e, { type: 'step' }> => e.type === 'step',
    );
    expect(respSteps[0].description).toBe('Thinking');
    expect(respSteps[0].outcome).toBe('success');
  });

  it('Back-to-back ThoughtStreamed: prior Thinking resolves, latest stays pending', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, '2026-05-24T19:00:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed', text: 'Context: 100 tokens, 1 messages' } as ThreadEvent, '2026-05-24T19:00:01Z');
    handleEvent(map, 't', 3, { type: 'ThoughtStreamed', text: 'Context: 200 tokens, 2 messages' } as ThreadEvent, '2026-05-24T19:00:02Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const steps = exchangeSteps(exchanges[0],  true);
    expect(steps).toHaveLength(2);
    expect(steps[0].outcome).toBe('success');
    expect(steps[1].outcome).toBe('pending');

    const respSteps = exchangeResponseEvents(exchanges[0]).filter(
      (e): e is Extract<typeof e, { type: 'step' }> => e.type === 'step',
    );
    expect(respSteps).toHaveLength(2);
    expect(respSteps[0].outcome).toBe('success');
    expect(respSteps[1].outcome).toBe('pending');
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
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    const fullCmd = 'cd /Users/me/projects/lucidos && git log --oneline -50 | head -20';
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'show recent commits' } as ThreadEvent, '2026-04-30T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'ToolCalled', name: 'run_bash', args: { command: fullCmd }, description: 'Running: cd /Users/me/projects/lucid…log --oneline -50 | head -20...' } as ThreadEvent, '2026-04-30T10:00:01Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const respEvents = exchangeResponseEvents(exchanges[0]);
    const stepEvent = respEvents.find(e => e.type === 'step') as Extract<typeof respEvents[number], { type: 'step' }> | undefined;
    expect(stepEvent?.full).toBe(fullCmd);
  });

  it('exchangeResponseEvents stamps event.created on step events for the detail modal', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, '2026-04-30T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'ToolCalled', name: 'run_bash', args: { command: 'ls' } } as ThreadEvent, '2026-04-30T10:00:01Z');
    handleEvent(map, 't', 3, { type: 'ThoughtStreamed' } as ThreadEvent, '2026-04-30T10:00:02Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const respEvents = exchangeResponseEvents(exchanges[0]);
    const steps = respEvents.filter((e): e is Extract<typeof e, { type: 'step' }> => e.type === 'step');
    expect(steps[0]?.created).toBe('2026-04-30T10:00:01Z');
    expect(steps[1]?.created).toBe('2026-04-30T10:00:02Z');
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

  it('fullCommandForCCTool returns the un-elided arg for common Claude Code tools', () => {
    expect(fullCommandForCCTool('Read', { file_path: '/src/lib.rs' })).toBe('/src/lib.rs');
    expect(fullCommandForCCTool('Edit', { file_path: '/src/lib.rs' })).toBe('/src/lib.rs');
    expect(fullCommandForCCTool('MultiEdit', { file_path: '/src/lib.rs' })).toBe('/src/lib.rs');
    expect(fullCommandForCCTool('Write', { file_path: '/src/lib.rs' })).toBe('/src/lib.rs');
    expect(fullCommandForCCTool('NotebookEdit', { file_path: '/n.ipynb' })).toBe('/n.ipynb');
    expect(fullCommandForCCTool('Bash', { command: 'cd /tmp && ls -la | head' })).toBe('cd /tmp && ls -la | head');
    expect(fullCommandForCCTool('WebFetch', { url: 'https://example.com/path' })).toBe('https://example.com/path');
    expect(fullCommandForCCTool('Glob', { pattern: '**/*.tsx' })).toBe('**/*.tsx');
    expect(fullCommandForCCTool('Grep', { pattern: 'TODO' })).toBe('TODO');
    expect(fullCommandForCCTool('WebSearch', { query: 'rust async' })).toBe('rust async');
    expect(fullCommandForCCTool('Agent', { description: 'short', prompt: 'do the long thing' })).toBe('do the long thing');
    expect(fullCommandForCCTool('Skill', { skill: 'foo' })).toBe('foo');
    expect(fullCommandForCCTool('UnknownTool', { x: 'y' })).toBeUndefined();
    expect(fullCommandForCCTool('Bash', null)).toBeUndefined();
    expect(fullCommandForCCTool('Bash', { command: 123 })).toBeUndefined();
  });

  it('fullCommandForCCTool formats TodoWrite as a status-prefixed list (active form for in-progress)', () => {
    const todos = [
      { content: 'Write tests', activeForm: 'Writing tests', status: 'completed' },
      { content: 'Implement feature', activeForm: 'Implementing feature', status: 'in_progress' },
      { content: 'Update docs', activeForm: 'Updating docs', status: 'pending' },
    ];
    expect(fullCommandForCCTool('TodoWrite', { todos })).toBe(
      '[x] Write tests\n[~] Implementing feature\n[ ] Update docs',
    );
    expect(fullCommandForCCTool('TodoWrite', { todos: [] })).toBeUndefined();
    expect(fullCommandForCCTool('TodoWrite', { todos: 'nope' })).toBeUndefined();
    expect(fullCommandForCCTool('TodoWrite', {})).toBeUndefined();
  });

  it('fullCommandForCCTool formats Codex todo_list as a checkbox list (TodoWrite parity)', () => {
    const items = [
      { text: 'Map the code', completed: true },
      { text: 'Fix the bug', completed: false },
    ];
    expect(fullCommandForCCTool('todo_list', { items })).toBe(
      '[x] Map the code\n[ ] Fix the bug',
    );
    expect(fullCommandForCCTool('todo_list', { items: [] })).toBeUndefined();
    expect(fullCommandForCCTool('todo_list', { items: 'nope' })).toBeUndefined();
    expect(fullCommandForCCTool('todo_list', {})).toBeUndefined();
  });

  it('fullCommandForCCTool gives a Codex step the un-elided value a Claude Code step gets', () => {
    expect(fullCommandForCCTool('command_execution', { command: '/bin/zsh -lc "cd /tmp && ls -la | head"' }))
      .toBe('cd /tmp && ls -la | head');
    expect(fullCommandForCCTool('Bash', { command: 'cd /tmp && ls -la | head' }))
      .toBe(fullCommandForCCTool('command_execution', { command: '/bin/zsh -lc "cd /tmp && ls -la | head"' }));
    expect(fullCommandForCCTool('command_execution', {})).toBeUndefined();

    expect(fullCommandForCCTool('file_change', {
      changes: [
        { kind: { type: 'update' }, path: '/w/src/lib.rs' },
        { kind: { type: 'delete' }, path: '/w/e2e/probe.spec.ts' },
      ],
    })).toBe('Edit /w/src/lib.rs\nDelete /w/e2e/probe.spec.ts');
    expect(fullCommandForCCTool('file_change', {
      changes: [{ kind: { type: 'update' }, path: '/w/a.ts' }, { kind: { type: 'add' } }],
    })).toBeUndefined();
    expect(fullCommandForCCTool('file_change', { changes: [] })).toBeUndefined();

    expect(fullCommandForCCTool('web_search', { query: 'rust async' })).toBe('rust async');
  });


  it('describeCCTool reads a Codex shell step like a Claude Code one', () => {
    const bare = describeCCTool('Bash', { command: 'git log --oneline -20' });
    expect(bare).toBe('Run git log --oneline -20');
    for (const wrapped of [
      '/bin/zsh -lc "git log --oneline -20"',
      "/bin/bash -lc 'git log --oneline -20'",
      'sh -c "git log --oneline -20"',
    ]) {
      expect(describeCCTool('command_execution', { command: wrapped })).toBe(bare);
    }
    expect(describeCCTool('command_execution', {})).toBe('Run command');
  });

  it('describeCCTool never unwraps a Claude Code Bash call', () => {
    expect(describeCCTool('Bash', { command: 'bash -lc "make lint"' }))
      .toBe('Run bash -lc "make lint"');
  });

  it('shellScriptBody declines anything it cannot read', () => {
    for (const untouched of [
      'cargo test --locked',
      'shellcheck -x scripts/e2e.sh',
      '/usr/local/bin/pushd -c \'x\'',
      'zsh --version',
      'bash -x script.sh',
      'zsh -lc "a" && "b"',
      'zsh -lc "git status',
      'zsh -lc',
      'zsh',
      '',
    ]) {
      expect(shellScriptBody(untouched)).toBe(untouched);
    }
  });

  it('shellScriptBody unescapes the quoting style it stripped', () => {
    expect(shellScriptBody("/bin/zsh -lc 'echo '\\''hi'\\'' > out'")).toBe("echo 'hi' > out");
    expect(shellScriptBody('/bin/zsh -lc "rg -n \\"pat\\" src"')).toBe('rg -n "pat" src');
    expect(shellScriptBody('/bin/zsh -lc "printf \'%s\\\\n\' x"')).toBe("printf '%s\\n' x");
    expect(shellScriptBody('zsh -lc "grep \'\\d+\' f"')).toBe("grep '\\d+' f");
  });

  it('shellScriptBody keeps the wrapper on a multiword unquoted script', () => {
    expect(shellScriptBody('zsh -lc git status')).toBe('zsh -lc git status');
    expect(describeCCTool('command_execution', { command: 'zsh -lc git status' }))
      .toBe('Run zsh -lc git status');
    expect(shellScriptBody('zsh -lc ls')).toBe('ls');
    expect(shellScriptBody('zsh -lc "git status"  ')).toBe('git status');
  });

  it('shellScriptBody handles the two real Codex shapes, one by reading it and one by declining', () => {
    expect(shellScriptBody('/bin/zsh -lc "rg -n \\"detailsExpanded|stepsExpanded\\" src -g \'*.ts\' | head -40; git status --short"'))
      .toBe('rg -n "detailsExpanded|stepsExpanded" src -g \'*.ts\' | head -40; git status --short');

    const concatenated = `/bin/zsh -lc 'if [ -e "$LOCK" ]; then sed -n '"'1,20p' \\""'$LOCK"; else echo MISSING; fi'`;
    expect(shellScriptBody(concatenated)).toBe(concatenated);
  });

  it('describeCCTool names the file a Codex change touches, in Claude Code vocabulary', () => {
    const label = (changes: unknown) => describeCCTool('file_change', { changes });

    expect(label([{ kind: { type: 'add' }, path: '/w/src/new.ts' }])).toBe('Write new.ts');
    expect(label([{ kind: { type: 'update', move_path: null }, path: '/w/src/lib.rs' }])).toBe('Edit lib.rs');
    expect(label([{ kind: { type: 'delete' }, path: '/w/e2e/probe.spec.ts' }])).toBe('Delete probe.spec.ts');
    expect(label([{ kind: 'update', path: '/w/src/lib.rs' }])).toBe('Edit lib.rs');
    expect(describeCCTool('Edit', { file_path: '/w/src/lib.rs' })).toBe('Edit lib.rs');
  });

  it('describeCCTool only claims one verb when the whole Codex patch agrees', () => {
    const label = (changes: unknown) => describeCCTool('file_change', { changes });

    expect(label([
      { kind: { type: 'update' }, path: '/w/a.ts' },
      { kind: { type: 'update' }, path: '/w/b.ts' },
      { kind: { type: 'update' }, path: '/w/c.ts' },
    ])).toBe('Edit 3 files');
    expect(label([
      { kind: { type: 'add' }, path: '/w/a.ts' },
      { kind: { type: 'delete' }, path: '/w/b.ts' },
    ])).toBe('Change 2 files');
    expect(label([{ path: '/w/a.ts' }])).toBe('Change a.ts');
    expect(label([{ kind: 'constructor', path: '/w/a.ts' }])).toBe('Change a.ts');
    expect(label([{ kind: { type: 'add' } }])).toBe('Write 1 file');
    expect(label([])).toBe('Apply file changes');
    expect(describeCCTool('file_change', {})).toBe('Apply file changes');
  });

  it('describeCCTool labels the remaining shared and Codex tools like Rust does', () => {
    expect(describeCCTool('TodoWrite', {})).toBe('Update plan');
    expect(describeCCTool('todo_list', {})).toBe('Update plan');
    expect(describeCCTool('ExitPlanMode', {})).toBe('Present plan for approval');
    expect(describeCCTool('web_search', { query: 'rust lifetimes' })).toBe("Search 'rust lifetimes'");
    expect(describeCCTool('WebSearch', { query: 'rust lifetimes' })).toBe("Search 'rust lifetimes'");
    expect(describeCCTool('mcp__example_server__create_issue', {})).toBe('MCP: create_issue');
    expect(describeCCTool('mcp__weird', {})).toBe('MCP: mcp__weird');
    expect(describeCCTool('CustomTool', {})).toBe('CustomTool');
  });

  it('exchangeResponseEvents stamps full path on CodingAgentToolCalled steps for hover tooltip', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t', thread]]);
    const fullPath = '/Users/me/projects/lucidos/crates/lucidos-app/src/store/thread-events.ts';
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'open it' } as ThreadEvent, '2026-05-09T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: fullPath }, description: 'Read thread-events.ts' } as ThreadEvent, '2026-05-09T10:00:01Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const respEvents = exchangeResponseEvents(exchanges[0]);
    const stepEvent = respEvents.find(e => e.type === 'step') as Extract<typeof respEvents[number], { type: 'step' }> | undefined;
    expect(stepEvent?.full).toBe(fullPath);
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
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);

    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'find the file' } as ThreadEvent, '2026-04-13T19:30:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed' } as ThreadEvent, '2026-04-13T19:30:01Z');
    handleEvent(map, 't', 3, { type: 'ToolCalled', name: 'bash', args: { command: 'find . -name "Foo.tsx"' } } as ThreadEvent, '2026-04-13T19:30:02Z');
    handleEvent(map, 't', 4, { type: 'MessageReceived', text: 'stop, I found it' } as ThreadEvent, '2026-04-13T19:30:10Z');
    handleEvent(map, 't', 5, { type: 'ToolResult', name: 'bash', result: 'ok' } as ThreadEvent, '2026-04-13T19:30:15Z');
    handleEvent(map, 't', 6, { type: 'ResponseCanceled' } as ThreadEvent, '2026-04-13T19:30:15Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[2].userEvent.type).toBe('ResponseCanceled');

    const stepsAsLast = exchangeSteps(exchanges[0], true);
    expect(stepsAsLast.filter(s => s.outcome === 'pending')).toHaveLength(1);

    const ex1Steps = exchangeSteps(exchanges[0],  false,  true);
    expect(ex1Steps.filter(s => s.outcome === 'pending')).toHaveLength(0);

    const ex1Events = exchangeResponseEvents(exchanges[0],  false,  true);
    const pendingEvents = ex1Events.filter(e => e.type === 'step' && (e as { outcome: StepOutcome }).outcome === 'pending');
    expect(pendingEvents).toHaveLength(0);
  });
});




describe('exchangeStatus — CC follow-up happy path', () => {
  it('normal Claude Code session: message → work → idle = done', () => {
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
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      { seq: 5, event: { type: 'CodingAgentTextStreamed', text: 'Done with initial analysis' } as ThreadEvent, created: '2026-04-12T10:00:04Z' },
      { seq: 6, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:05Z' },
      { seq: 7, event: { type: 'MessageReceived', text: 'now also fix the tests', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 8, event: { type: 'CodingAgentUserMessageSent', text: 'now also fix the tests' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 9, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 10, event: { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
      { seq: 11, event: { type: 'CodingAgentTextStreamed', text: 'Tests fixed!' } as ThreadEvent, created: '2026-04-12T10:01:03Z' },
      { seq: 12, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:01:04Z' },
    ]);
    expect(exchanges).toHaveLength(2);
    expect(statuses[0]).toBe('done');
    expect(statuses[1]).toBe('done');
  });

  it('CC follow-up without CodingAgentUserMessageSent (new data path) = both done', () => {
    const { statuses, exchanges } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:05Z' },
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
      { seq: 1, event: { type: 'MessageReceived', text: 'first', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'second', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentUserMessageSent', text: 'second' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 6, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
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
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'also fix tests', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentUserMessageSent', text: 'also fix tests' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 6, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 7, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
    ]);
    expect(exchanges).toHaveLength(3);
    expect(statuses[0]).toBe('done');
    expect(statuses[1]).toBe('aborted');
    expect(exchanges[2].userEvent.type).toBe('ResponseAborted');
  });

  it('CC crash on follow-up: ResponseAborted opens its own boundary exchange', () => {
    const { exchanges } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
    ]);
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
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up that got lost', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
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
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 6, event: { type: 'ResponseAborted' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
      { seq: 7, event: { type: 'SessionEnded', reason: 'completed' } as ThreadEvent, created: '2026-04-12T10:01:03Z' },
    ]);
    expect(statuses[1]).toBe('aborted');
  });
});

describe('CodingAgentThoughtStreamed reasoning rendering', () => {
  it('accumulates reasoning deltas into the Thinking step and resolves on text', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'fix it' } as ThreadEvent, '2026-04-04T10:00:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-04T10:00:01Z');
    handleEvent(map, 'thread-1', 3, { type: 'CodingAgentPromptSent', text: 'fix it' } as ThreadEvent, '2026-04-04T10:00:02Z');
    handleEvent(map, 'thread-1', 4, { type: 'CodingAgentThoughtStreamed', text: 'Let me ' } as ThreadEvent, '2026-04-04T10:00:03Z');
    handleEvent(map, 'thread-1', 5, { type: 'CodingAgentThoughtStreamed', text: 'check the tokens.' } as ThreadEvent, '2026-04-04T10:00:04Z');
    handleEvent(map, 'thread-1', 6, { type: 'CodingAgentTextStreamed', text: 'Done!' } as ThreadEvent, '2026-04-04T10:00:05Z');
    handleEvent(map, 'thread-1', 7, { type: 'CodingAgentIdled' } as ThreadEvent, '2026-04-04T10:00:06Z');

    const exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    const events = exchangeResponseEvents(exchanges[0]);
    const thinking = events.find(e => e.type === 'step' && e.description === 'Thinking') as
      | { thinkingText?: string; outcome: StepOutcome }
      | undefined;
    expect(thinking).toBeDefined();
    expect(thinking!.thinkingText).toBe('Let me check the tokens.');
    expect(thinking!.outcome).toBe('success');
  });

  it('opens a Thinking step from reasoning when no CodingAgentPromptSent fired (resumed initial prompt)', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'redirect' } as ThreadEvent, '2026-04-04T10:00:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-04T10:00:01Z');
    handleEvent(map, 'thread-1', 3, { type: 'CodingAgentThoughtStreamed', text: 'Reasoning hard…' } as ThreadEvent, '2026-04-04T10:00:02Z');

    const exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    const events = exchangeResponseEvents(exchanges[0]);
    const thinking = events.find(e => e.type === 'step' && e.description === 'Thinking') as
      | { thinkingText?: string; outcome: StepOutcome }
      | undefined;
    expect(thinking).toBeDefined();
    expect(thinking!.thinkingText).toBe('Reasoning hard…');
    expect(thinking!.outcome).toBe('pending');
  });
});

