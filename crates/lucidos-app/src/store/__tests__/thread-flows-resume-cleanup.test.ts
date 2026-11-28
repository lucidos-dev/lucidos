import { describe, it, expect, beforeEach } from 'vitest';
import { getExchanges, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { abortPromisesAutoResume, changeReadyToReview, abortTookEngineDown, continuableAbortIndex, exchangeResponseEvents, exchangeStatus, exchangeSteps, hasRenderableResponseContent, responseAbortedSummary, type ThreadEvent } from '../thread-events';
import { isActive } from '../exchange-status';
import { displaySection } from '../../generated/thread-lifecycle';
import type { StepOutcome } from '../types';
import { isRenderedThreadIdle, isThreadQuiescent } from '../store';

beforeEach(resetSeqCounter);

describe('CC stale resume — SessionEnded(stale_resume) must not cause aborted status', () => {
  const now = Date.now();
  const t = (offset: number) => new Date(now + offset).toISOString();

  it('mid-resume: SessionEnded(stale_resume) followed by new SessionStarted is coding-agent-working, not aborted', () => {
    resetSeqCounter();
    const { map, id } = makeThread('stale-resume-1', 'running');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: { file: 'foo.rs' }, created: t(-280000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-279000) },
      { type: 'ResponseGenerated', created: t(-270000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-269000) },
      { type: 'MessageReceived', text: 'include the ios suite too', channel: 'claude_code', created: t(-60000) },
      { type: 'SessionEnded', reason: 'stale_resume', created: t(-59000) },
      { type: 'SessionStarted', session_id: 's2', created: t(-58000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: { file: 'ios.rs' }, created: t(-50000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    const followUp = exchanges[1];
    const status = exchangeStatus(followUp, '', true, false, true);
    expect(status).not.toBe('aborted');
    expect(status).toBe('coding-agent-working');
  });

  it('stale_resume only (before retry SessionStarted arrives) is not aborted', () => {
    resetSeqCounter();
    const { map, id } = makeThread('stale-resume-2', 'running');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      { type: 'ResponseGenerated', created: t(-270000) },
      { type: 'CodingAgentIdled', has_changes: false, created: t(-269000) },
      { type: 'MessageReceived', text: 'also fix bar', channel: 'claude_code', created: t(-60000) },
      { type: 'SessionEnded', reason: 'stale_resume', created: t(-59000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    const followUp = exchanges[1];
    const status = exchangeStatus(followUp, '', true, false, true);
    expect(status).not.toBe('aborted');
  });

  it('thread status stays running after SessionEnded(stale_resume)', () => {
    resetSeqCounter();
    const { map, id } = makeThread('stale-status-1', 'running');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      { type: 'SessionEnded', reason: 'stale_resume', created: t(-298000) },
    ]);

    const thread = map.get(id)!;
    expect(thread.meta.status).toBe('running');
  });

  it('thread status stays running through full stale resume → retry sequence', () => {
    resetSeqCounter();
    const { map, id } = makeThread('stale-status-2', 'running');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      { type: 'SessionEnded', reason: 'stale_resume', created: t(-298000) },
      { type: 'SessionStarted', session_id: 's2', created: t(-297000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: { file: 'foo.rs' }, created: t(-290000) },
    ]);

    const thread = map.get(id)!;
    expect(thread.meta.status).toBe('running');
    expect(displaySection(thread.meta.section, thread.meta.status, thread.meta.saved, thread.meta.activeChildrenCount > 0, changeReadyToReview(thread.meta), false, false)).toBe('current');
  });
});


describe('stale exchange recovery (incomplete last exchange)', () => {
  const t = (ms: number) => new Date(Date.now() + ms).toISOString();

  it('chat thread: last exchange with ToolCalled but no terminal event settles done when thread is idle', () => {
    resetSeqCounter();
    const { map, id } = makeThread('stale-exchange-1', 'idle');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the workflow app', channel: 'chat', created: t(-300000) },
      { type: 'TextStreamed', text: 'Let me check...', created: t(-299000) },
      { type: 'ToolCalled', name: 'read_file', args: { path: 'index.html' }, description: 'Reading index.html...', created: t(-298000) },
      { type: 'ToolResult', name: 'read_file', result: '<html>...', created: t(-297000) },
      { type: 'ResponseGenerated', text: 'Fixed it.', created: t(-296000) },
      { type: 'MessageReceived', text: 'Doesnt work', channel: 'chat', created: t(-200000) },
      { type: 'TextStreamed', text: 'Let me investigate...', created: t(-199000) },
      { type: 'ToolCalled', name: 'run_python', args: { code: 'import os' }, description: 'Running Python code...', created: t(-198000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');

    const lastExchange = exchanges[1];
    const threadIdle = true;
    const status = exchangeStatus(lastExchange, '', true, false, false, threadIdle);
    expect(status).toBe('done');

    const steps = exchangeSteps(lastExchange, true, threadIdle);
    const pendingSteps = steps.filter(s => s.outcome === 'pending');
    expect(pendingSteps).toHaveLength(0);
  });

  it('chat thread: last exchange with only TextStreamed (no tools) settles done when thread is idle', () => {
    resetSeqCounter();
    const { map, id } = makeThread('stale-exchange-2', 'idle');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Hello', channel: 'chat', created: t(-300000) },
      { type: 'ResponseGenerated', text: 'Hi!', created: t(-299000) },
      { type: 'MessageReceived', text: 'How are you?', channel: 'chat', created: t(-200000) },
      { type: 'TextStreamed', text: 'I am doing w', created: t(-199000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    const lastExchange = exchanges[1];
    const threadIdle = true;
    const status = exchangeStatus(lastExchange, '', true, false, false, threadIdle);
    expect(status).toBe('done');
  });

  it('exchange with streaming buffer is NOT aborted even when thread is idle', () => {
    resetSeqCounter();
    const { map, id } = makeThread('stale-exchange-3', 'idle');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Hello', channel: 'chat', created: t(-200000) },
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python...', created: t(-199000) },
    ]);

    const exchanges = getExchanges(map, id);
    const status = exchangeStatus(exchanges[0], 'partial text arriving...', true, false, false, true);
    expect(status).toBe('streaming');
  });

  it('non-idle thread with incomplete exchange is still streaming (not aborted)', () => {
    resetSeqCounter();
    const { map, id } = makeThread('stale-exchange-4', 'running');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Hello', channel: 'chat', created: t(-200000) },
      { type: 'TextStreamed', text: 'Working on it...', created: t(-199000) },
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python...', created: t(-198000) },
    ]);

    const exchanges = getExchanges(map, id);
    const status = exchangeStatus(exchanges[0], '', true, false, false, false);
    expect(status).toBe('streaming');
  });

  it('chat follow-up absorbed via PromptInjected is NOT aborted once thread idles', () => {
    resetSeqCounter();
    const { map, id } = makeThread('injection-folded-1', 'idle');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'ferdig', channel: 'chat', created: t(-300000), event_id: 'msg-A' },
      { type: 'ThoughtStreamed', text: 'analyzing', request_event_id: 'msg-A', created: t(-299000) } as ThreadEvent,
      { type: 'ToolCalled', name: 'sql_query', args: {}, description: 'Querying...', request_event_id: 'msg-A', created: t(-298000) } as ThreadEvent,
      { type: 'ToolResult', name: 'sql_query', result: 'ok', request_event_id: 'msg-A', created: t(-297000) } as ThreadEvent,
      { type: 'MessageReceived', text: 'men kan ikke ha dette hele tiden', channel: 'chat', created: t(-296000), event_id: 'msg-B' },
      { type: 'ToolCalled', name: 'check_creds', args: {}, description: 'Checking creds...', request_event_id: 'msg-A', created: t(-295000) } as ThreadEvent,
      { type: 'ToolResult', name: 'check_creds', result: 'ok', request_event_id: 'msg-A', created: t(-294000) } as ThreadEvent,
      { type: 'PromptInjected', text: 'men kan ikke ha dette hele tiden', injected_message_id: 'msg-B', request_event_id: 'msg-A', created: t(-293000) } as ThreadEvent,
      { type: 'TextStreamed', text: 'Combined answer', request_event_id: 'msg-A', created: t(-292000) } as ThreadEvent,
      { type: 'ResponseGenerated', text: 'Combined answer', request_event_id: 'msg-A', created: t(-291000) } as ThreadEvent,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect((exchanges[0].userEvent as { text: string }).text).toBe('ferdig');
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual([
      'ThoughtStreamed', 'ToolCalled', 'ToolResult', 'ToolCalled', 'ToolResult',
    ]);
    expect(exchangeStatus(exchanges[0], '', false, false, false, true)).toBe('interrupted');

    expect(exchanges[1].userEvent.type).toBe('MessageReceived');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('men kan ikke ha dette hele tiden');
    expect(exchanges[1].steps.map(s => s.event.type)).toEqual([
      'PromptInjected', 'TextStreamed', 'ResponseGenerated',
    ]);
    expect(exchangeStatus(exchanges[1], '', true, false, false, true)).toBe('done');
  });
});

describe('CC SessionEnded(changes_proposed) without preceding CodingAgentIdled', () => {
  const t = (offset: number) => new Date(Date.now() + offset).toISOString();

  it('treats SessionEnded(changes_proposed) as terminal even without CodingAgentIdled', () => {
    resetSeqCounter();
    const { map, id } = makeThread('cc-changes-no-idle', 'waiting');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'merge in main', channel: 'claude_code', created: t(-200000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-199000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: { file: 'foo.rs' }, created: t(-180000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-179000) },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'cargo test' }, created: t(-160000) },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'Exit code 137', created: t(-150000) },
      { type: 'SessionEnded', reason: 'changes_proposed', created: t(-149000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);

    const status = exchangeStatus(exchanges[0], '', true, false, true);
    expect(status).not.toBe('coding-agent-working');
    expect(status).toBe('done');
  });

  it('SessionEnded(changes_proposed) WITH preceding CodingAgentIdled is also done', () => {
    resetSeqCounter();
    const { map, id } = makeThread('cc-changes-with-idle', 'waiting');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'do the thing', channel: 'claude_code', created: t(-200000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-199000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: { file: 'foo.rs' }, created: t(-180000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-179000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-170000) },
      { type: 'SessionEnded', reason: 'changes_proposed', created: t(-169000) },
    ]);

    const exchanges = getExchanges(map, id);
    const status = exchangeStatus(exchanges[0], '', true, false, true);
    expect(status).toBe('done');
  });
});

describe('chat follow-up while parent loop still running', () => {
  const t = (offset: number) => new Date(Date.now() + offset).toISOString();

  it('parent mid-flight is NOT done and pending step stays a spinner when follow-up arrives', () => {
    resetSeqCounter();
    const { map, id } = makeThread('parent-midflight-1', 'running');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the script', channel: 'chat', created: t(-30000), event_id: 'parent-mr' },
      { type: 'ThoughtStreamed', text: 'analyzing', request_event_id: 'parent-mr', created: t(-29000) } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python code...', request_event_id: 'parent-mr', created: t(-25000) } as ThreadEvent,
      { type: 'ToolResult', name: 'run_python', result: 'ok', request_event_id: 'parent-mr', created: t(-24000) } as ThreadEvent,
      { type: 'ThoughtStreamed', text: 'now python again', request_event_id: 'parent-mr', created: t(-22000) } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python code...', request_event_id: 'parent-mr', created: t(-20000) } as ThreadEvent,
      { type: 'MessageReceived', text: 'Uuhh fix the script?', channel: 'chat', created: t(-10000), event_id: 'followup-mr' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    const parentStatus = exchangeStatus(exchanges[0], '',  false,  false,  false,  false);
    expect(parentStatus).not.toBe('done');

    const events = exchangeResponseEvents(exchanges[0],  false);
    const pythonSteps = events.filter(e => e.type === 'step' && /python/i.test((e as { description?: string }).description ?? ''));
    expect(pythonSteps).toHaveLength(2);
    const lastPython = pythonSteps[pythonSteps.length - 1] as { outcome: StepOutcome };
    expect(lastPython.outcome).toBe('pending');
  });

  it('production-style absorbed PromptInjected: follow-up status is done, not aborted', () => {
    resetSeqCounter();
    const { map, id } = makeThread('injection-prod', 'idle');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: "No way! I'm not going into the terminal, pls fix", channel: 'chat', created: '2026-05-04T11:51:39.438Z', event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31' },
      { type: 'MemorySearched', results: 5, request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:51:54.537Z' } as ThreadEvent,
      { type: 'ThoughtStreamed', text: '', context_tokens:5147, context_messages: 1, request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:51:54.557Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_bash', args: {}, description: 'Running bash...', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:52:00.358Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'run_bash', result: 'ok', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:52:00.403Z' } as ThreadEvent,
      { type: 'ThoughtStreamed', text: '', context_tokens:7242, context_messages: 3, request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:52:00.410Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python code...', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:52:17.583Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'run_python', result: 'ok', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:52:17.789Z' } as ThreadEvent,
      { type: 'ThoughtStreamed', text: '', context_tokens:7634, context_messages: 5, request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:52:17.793Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python code...', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:52:30.133Z' } as ThreadEvent,
      { type: 'MessageReceived', text: 'Uuhh fix the script?', channel: 'chat', created: '2026-05-04T11:52:38.205Z', event_id: 'a7d179ab-f451-4ff7-89dd-61ed413aaa88' },
      { type: 'ToolResult', name: 'run_python', result: 'ok', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:01.080Z' } as ThreadEvent,
      { type: 'PromptInjected', text: 'Uuhh fix the script?', mode: 'human', injected_message_id: 'a7d179ab-f451-4ff7-89dd-61ed413aaa88', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:01.092Z' } as ThreadEvent,
      { type: 'ThoughtStreamed', text: '', context_tokens:15806, context_messages: 8, request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:01.098Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'emit_event', args: {}, description: 'Emitting event…', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:27.707Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'emit_event', result: 'ok', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:27.731Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_claude', args: {}, description: 'Executing Claude Code…', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:27.737Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'run_claude', result: 'ok', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:27.768Z' } as ThreadEvent,
      { type: 'ThoughtStreamed', text: '', context_tokens:16425, context_messages: 10, request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:27.776Z' } as ThreadEvent,
      { type: 'TextStreamed', text: 'Released first…', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:31.886Z' } as ThreadEvent,
      { type: 'ResponseGenerated', text: 'Released first…', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:31.893Z' } as ThreadEvent,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].steps.length).toBeGreaterThan(0);
    expect(exchanges[0].steps.some(s => s.event.type === 'ResponseGenerated')).toBe(false);
    const followupTypes = exchanges[1].steps.map(s => s.event.type);
    expect(followupTypes[0]).toBe('PromptInjected');
    expect(followupTypes).toContain('ResponseGenerated');

    const followupStatus = exchangeStatus(exchanges[1], '',  true,  false,  false,  true);
    expect(followupStatus).toBe('done');
  });

  it('empty non-last chat exchange does not lock the next exchange into queued', () => {
    resetSeqCounter();
    const { map, id } = makeThread('empty-non-last-1', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'A done', channel: 'chat', created: t(-30000) },
      { type: 'ResponseGenerated', text: 'A reply', created: t(-29000) } as ThreadEvent,
      { type: 'MessageReceived', text: 'B empty', channel: 'chat', created: t(-20000) },
      { type: 'MessageReceived', text: 'C running', channel: 'chat', created: t(-10000) },
      { type: 'ToolCalled', name: 'run_bash', args: {}, description: 'Running bash...', created: t(-9000) } as ThreadEvent,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);
    const bStatus = exchangeStatus(exchanges[1], '',  false,  false,  false,  false);
    expect(bStatus).toBe('done');
    expect(isActive(bStatus)).toBe(false);
  });

  it('follow-up with absorbed PromptInjected is done after parent ResponseGenerated, not aborted', () => {
    resetSeqCounter();
    const { map, id } = makeThread('followup-absorbed-real', 'idle');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the script', channel: 'chat', created: t(-30000), event_id: 'parent-mr-2' },
      { type: 'ThoughtStreamed', text: 'analyzing', request_event_id: 'parent-mr-2', created: t(-29000) } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python code...', request_event_id: 'parent-mr-2', created: t(-25000) } as ThreadEvent,
      { type: 'MessageReceived', text: 'Uuhh fix the script?', channel: 'chat', created: t(-22000), event_id: 'followup-mr-2' },
      { type: 'ToolResult', name: 'run_python', result: 'ok', request_event_id: 'parent-mr-2', created: t(-15000) } as ThreadEvent,
      { type: 'PromptInjected', text: 'Uuhh fix the script?', mode: 'human', injected_message_id: 'followup-mr-2', request_event_id: 'parent-mr-2', created: t(-14990) } as ThreadEvent,
      { type: 'ThoughtStreamed', text: 'now incorporating user note', request_event_id: 'parent-mr-2', created: t(-14000) } as ThreadEvent,
      { type: 'TextStreamed', text: 'Released first…', request_event_id: 'parent-mr-2', created: t(-2000) } as ThreadEvent,
      { type: 'ResponseGenerated', text: 'Released first…', request_event_id: 'parent-mr-2', created: t(-1000) } as ThreadEvent,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    expect(exchanges[1].userEvent.type).toBe('MessageReceived');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('Uuhh fix the script?');
    expect(exchanges[1].steps.map(s => s.event.type)).toEqual([
      'PromptInjected', 'ThoughtStreamed', 'TextStreamed', 'ResponseGenerated',
    ]);

    const followupStatus = exchangeStatus(exchanges[1], '',  true,  false,  false,  true);
    expect(followupStatus).toBe('done');
  });

  it('production thread: absorbed PromptInjected follow-up resolves to done', () => {
    resetSeqCounter();
    const { map, id } = makeThread('audit-prod', 'idle');
    const MR1 = 'c4f1ef84-48c2-4f5a-8319-e79d954c3722';
    const MR2 = 'ec64bf8b-f37e-4b22-beb5-e76a340f9175';
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'La oss droppe påminnelser trigger - og fjerne ref til den', channel: 'chat', created: '2026-05-05T06:11:30.599Z', event_id: MR1 },
      { type: 'MemorySearched', results: 60, request_event_id: MR1, created: '2026-05-05T06:11:32.094Z' } as ThreadEvent,
      { type: 'ThoughtStreamed', text: '', context_tokens: 6934, context_messages: 1, request_event_id: MR1, created: '2026-05-05T06:11:32.137Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'list_triggers', args: {}, description: 'Listing triggers...', request_event_id: MR1, created: '2026-05-05T06:11:35.045Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'list_triggers', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:35.069Z' } as ThreadEvent,
      { type: 'ThoughtStreamed', text: '', context_tokens: 7668, context_messages: 3, request_event_id: MR1, created: '2026-05-05T06:11:35.090Z' } as ThreadEvent,
      { type: 'MessageReceived', text: 'for calendar altså', channel: 'chat', created: '2026-05-05T06:11:39.201Z', event_id: MR2 },
      { type: 'ToolCalled', name: 'delete_trigger', args: {}, description: 'Deleting trigger...', request_event_id: MR1, created: '2026-05-05T06:11:41.445Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'delete_trigger', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:41.479Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'delete_file', args: {}, description: 'Deleting file...', request_event_id: MR1, created: '2026-05-05T06:11:41.489Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'delete_file', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:41.544Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'delete_file', args: {}, description: 'Deleting file...', request_event_id: MR1, created: '2026-05-05T06:11:41.549Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'delete_file', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:41.568Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'grep_files', args: {}, description: 'Grepping...', request_event_id: MR1, created: '2026-05-05T06:11:41.572Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'grep_files', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:42.239Z' } as ThreadEvent,
      { type: 'PromptInjected', text: 'for calendar altså', mode: 'human', injected_message_id: MR2, request_event_id: MR1, created: '2026-05-05T06:11:42.244Z' } as ThreadEvent,
      { type: 'ThoughtStreamed', text: '', context_tokens: 8329, context_messages: 6, request_event_id: MR1, created: '2026-05-05T06:11:42.248Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'read_file', args: {}, description: 'Reading file...', request_event_id: MR1, created: '2026-05-05T06:11:47.792Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'read_file', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:47.802Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_bash', args: {}, description: 'Running bash...', request_event_id: MR1, created: '2026-05-05T06:11:47.807Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'run_bash', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:48.031Z' } as ThreadEvent,
      { type: 'ThoughtStreamed', text: '', context_tokens: 8630, context_messages: 8, request_event_id: MR1, created: '2026-05-05T06:11:48.041Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'edit_file', args: {}, description: 'Editing file...', request_event_id: MR1, created: '2026-05-05T06:11:56.694Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'edit_file', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:56.740Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'edit_file', args: {}, description: 'Editing file...', request_event_id: MR1, created: '2026-05-05T06:11:56.746Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'edit_file', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:56.770Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'grep_files', args: {}, description: 'Grepping...', request_event_id: MR1, created: '2026-05-05T06:11:56.774Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'grep_files', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:56.787Z' } as ThreadEvent,
      { type: 'ThoughtStreamed', text: '', context_tokens: 9013, context_messages: 10, request_event_id: MR1, created: '2026-05-05T06:11:56.791Z' } as ThreadEvent,
      { type: 'TextStreamed', text: 'Ferdig.', request_event_id: MR1, created: '2026-05-05T06:12:00.037Z' } as ThreadEvent,
      { type: 'ResponseGenerated', text: 'Ferdig.', request_event_id: MR1, created: '2026-05-05T06:12:00.046Z' } as ThreadEvent,
      { type: 'ThreadSaved', created: '2026-05-05T06:12:00.056Z' } as ThreadEvent,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('MessageReceived');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('for calendar altså');
    const followupTypes = exchanges[1].steps.map(s => s.event.type);
    expect(followupTypes[0]).toBe('PromptInjected');
    expect(followupTypes[followupTypes.length - 1]).toBe('ResponseGenerated');

    const followupStatus = exchangeStatus(exchanges[1], '',  true,  false,  false,  true);
    expect(followupStatus).toBe('done');
  });

  it('post-injection events route to the absorbed-into exchange, not the original request', () => {
    resetSeqCounter();
    const { map, id } = makeThread('injection-split', 'idle');
    const MR1 = 'aaaaaaaa-1111-1111-1111-111111111111';
    const MR2 = 'bbbbbbbb-2222-2222-2222-222222222222';
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'first', channel: 'chat', created: '2026-05-05T10:00:00.000Z', event_id: MR1 },
      { type: 'ThoughtStreamed', text: 'pre', request_event_id: MR1, created: '2026-05-05T10:00:01.000Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'pre_tool', args: {}, description: 'Pre tool...', request_event_id: MR1, created: '2026-05-05T10:00:02.000Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'pre_tool', result: 'ok', request_event_id: MR1, created: '2026-05-05T10:00:03.000Z' } as ThreadEvent,
      { type: 'MessageReceived', text: 'second', channel: 'chat', created: '2026-05-05T10:00:04.000Z', event_id: MR2 },
      { type: 'ToolCalled', name: 'in_flight', args: {}, description: 'In-flight tool...', request_event_id: MR1, created: '2026-05-05T10:00:05.000Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'in_flight', result: 'ok', request_event_id: MR1, created: '2026-05-05T10:00:06.000Z' } as ThreadEvent,
      { type: 'PromptInjected', text: 'second', mode: 'human', injected_message_id: MR2, request_event_id: MR1, created: '2026-05-05T10:00:07.000Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'post_tool', args: {}, description: 'Post tool...', request_event_id: MR1, created: '2026-05-05T10:00:08.000Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'post_tool', result: 'ok', request_event_id: MR1, created: '2026-05-05T10:00:09.000Z' } as ThreadEvent,
      { type: 'TextStreamed', text: 'Combined', request_event_id: MR1, created: '2026-05-05T10:00:10.000Z' } as ThreadEvent,
      { type: 'ResponseGenerated', text: 'Combined', request_event_id: MR1, created: '2026-05-05T10:00:11.000Z' } as ThreadEvent,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    const ex1Steps = exchanges[0].steps.map(s => s.event.type);
    expect(ex1Steps).toEqual([
      'ThoughtStreamed',
      'ToolCalled', 'ToolResult',
      'ToolCalled', 'ToolResult',
    ]);

    const ex2Steps = exchanges[1].steps.map(s => s.event.type);
    expect(ex2Steps).toEqual([
      'PromptInjected',
      'ToolCalled', 'ToolResult',
      'TextStreamed',
      'ResponseGenerated',
    ]);

    expect(exchangeStatus(exchanges[0], '',  false, false, false,  true)).toBe('interrupted');
    expect(exchangeStatus(exchanges[1], '',  true, false, false,  true)).toBe('done');
  });
});

describe('CC waiting_for_user_answer: trailing Thinking cleanup in non-last exchange', () => {
  it('strips stranded CodingAgentPromptSent Thinking when CC paused on AskUserQuestion', () => {
    const { map, id } = makeThread('cc-strand-1', 'idle');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'help with auth design', channel: 'claude_code', mode: 'human' } as any,
      { type: 'SessionStarted', session_id: 'cc-1', branch: 'claude-code/test' },
      { type: 'CodingAgentTextStreamed', text: 'Initial analysis…' },
      { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: 'cc-1', question: 'Pick approach:', options: [{ id: 'opt-0', label: 'A' }] },
      { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'opt-0' } },
      { type: 'CodingAgentPromptSent', text: '' },
      { type: 'CodingAgentToolResult', name: '', result: 'opt-0', tool_use_id: 'tu-1' },
      { type: 'CodingAgentTextStreamed', text: 'Pipeline accepted. Next:' },
      { type: 'MessageReceived', text: 'A provider can impl just some layers?', channel: 'claude_code', mode: 'human' } as any,
      { type: 'CodingAgentPromptSent', text: 'A provider can impl just some layers?' },
      { type: 'UserQuestionAsked', tool_use_id: 'tu-2', cc_session_id: 'cc-1', question: 'Next decision:', options: [{ id: 'opt-0', label: 'X' }] },
    ]);

    expect(map.get(id)!.meta.status).toBe('waiting_for_user_answer');
    const threadIdle = isThreadQuiescent(map.get(id)!.meta.status);
    expect(threadIdle).toBe(true);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(4);
    expect(exchanges[2].userEvent.type).toBe('MessageReceived');
    expect(exchanges[3].userEvent.type).toBe('UserQuestionAsked');

    const isPendingThinking = (e: { type: string; description?: string; outcome?: StepOutcome }) =>
      e.type === 'step' && e.description === 'Thinking' && e.outcome === 'pending';

    const e2Events = exchangeResponseEvents(exchanges[1],  false, threadIdle);
    expect(e2Events.filter(isPendingThinking)).toHaveLength(0);

    const e3Events = exchangeResponseEvents(exchanges[2],  false, threadIdle);
    expect(e3Events.filter(isPendingThinking)).toHaveLength(0);

    expect(exchanges[2].continuationMoved).toBe(true);
    const e3WhileRunning = exchangeResponseEvents(exchanges[2],  false,  false);
    expect(e3WhileRunning.filter(isPendingThinking)).toHaveLength(0);
  });
});

describe('coding-agent turn with no terminator must not read Working forever', () => {
  const now = Date.now();
  const t = (offset: number) => new Date(now + offset).toISOString();

  const seedOvertakenQuestion = (
    name: string,
    status: 'idle' | 'running' | 'waiting',
    trailingText = true,
  ) => {
    const { map, id } = makeThread(name, status);
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'iterate on the vision', channel: 'claude_code', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      { type: 'CodingAgentTextStreamed', text: 'Here are the options.', created: t(-290000) },
      { type: 'UserQuestionAsked', tool_use_id: 'toolu_park', cc_session_id: 's1', question: 'Which line?', options: [{ id: 'opt-0', label: 'A' }], created: t(-280000) },
      { type: 'CodingAgentToolResult', name: '', tool_use_id: 'toolu_park', result: 'The user doesn\'t want to proceed with this tool use.', created: t(-200000) },
      ...(trailingText
        ? [{ type: 'CodingAgentTextStreamed' as const, text: '\n\n', created: t(-199000) }]
        : []),
    ]);
    return getExchanges(map, id);
  };

  it('reads done once the thread has settled, not coding-agent-working', () => {
    resetSeqCounter();
    const exchanges = seedOvertakenQuestion('cc-no-terminator-1', 'idle');
    const last = exchanges[exchanges.length - 1];

    const status = exchangeStatus(last, '', true, false,  true,  true);
    expect(status).toBe('done');
    expect(isActive(status)).toBe(false);
  });

  it('still reads coding-agent-working while the thread is genuinely running', () => {
    resetSeqCounter();
    const exchanges = seedOvertakenQuestion('cc-no-terminator-2', 'running');
    const last = exchanges[exchanges.length - 1];

    const status = exchangeStatus(last, '', true, false,  true,  false);
    expect(status).toBe('coding-agent-working');
  });

  it('does not fire during the answer-to-resume gap (threadAwaitingAnswer)', () => {
    resetSeqCounter();
    const exchanges = seedOvertakenQuestion('cc-no-terminator-3', 'waiting');
    const last = exchanges[exchanges.length - 1];

    const status = exchangeStatus(
      last, '', true, false,  true,  true,  true,
    );
    expect(status).not.toBe('aborted');
    expect(status).toBe('coding-agent-working');
  });

  it('a lone rejected tool result reads done, not "Needs your answer"', () => {
    resetSeqCounter();
    const exchanges = seedOvertakenQuestion('cc-no-terminator-4', 'idle',  false);
    const last = exchanges[exchanges.length - 1];
    expect(last.questionOvertaken).toBe(true);

    const status = exchangeStatus(last, '', true, false,  true,  true);
    expect(status).toBe('done');
  });

  it('a live question card is untouched by the overtaken carve-out', () => {
    resetSeqCounter();
    const { map, id } = makeThread('cc-live-question', 'waiting');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'iterate on the vision', channel: 'claude_code', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      { type: 'UserQuestionAsked', tool_use_id: 'toolu_live', cc_session_id: 's1', question: 'Which line?', options: [{ id: 'opt-0', label: 'A' }], created: t(-280000) },
    ]);

    const exchanges = getExchanges(map, id);
    const last = exchanges[exchanges.length - 1];
    expect(last.questionOvertaken).toBe(false);
    expect(exchangeStatus(last, '', true, false, true, true, true)).toBe('awaiting-answer');
  });

  it('a properly terminated CC turn on an idle thread is still done', () => {
    resetSeqCounter();
    const { map, id } = makeThread('cc-terminated-1', 'idle');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: { file: 'foo.rs' }, created: t(-280000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-279000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-270000) },
    ]);

    const exchanges = getExchanges(map, id);
    const status = exchangeStatus(exchanges[0], '', true, false, true, true);
    expect(status).toBe('done');
  });
});

describe('an abort boundary must not read Working while the engine is down', () => {
  const now = Date.now();
  const t = (offset: number) => new Date(now + offset).toISOString();
  const device = { kind: 'device', device_id: 'd1' } as const;

  const seedTeardownBoundary = (name: string, proposed = false) => {
    const { map, id } = makeThread(name, 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'run the browser suite', channel: 'claude_code', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'git status' }, created: t(-290000) },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'clean', created: t(-289000) },
      ...(proposed
        ? [{ type: 'ChangeProposed' as const, change_id: 'c1', description: 'x', files: [], created: t(-285000) }]
        : []),
      { type: 'ResponseAborted', cause: 'engine_shutdown', actor: device, created: t(-200000) },
      { type: 'CodingAgentToolResult', name: '', result: "The user doesn't want to proceed with this tool use.", created: t(-199959) },
      { type: 'CodingAgentTextStreamed', text: '\n\n', created: t(-199948) },
    ] as any);
    return { map, id, exchanges: getExchanges(map, id) };
  };

  for (const [label, proposed] of [['no pending change', false], ['a proposed change', true]] as const) {
    it(`reads as settled, not Working, with ${label}`, () => {
      resetSeqCounter();
      const { map, id, exchanges } = seedTeardownBoundary(`cc-teardown-${proposed}`, proposed);

      const boundary = exchanges[exchanges.length - 1];
      expect(boundary.userEvent.type).toBe('ResponseAborted');
      expect(boundary.steps.length).toBeGreaterThan(0);

      const threadIdle = isRenderedThreadIdle(map.get(id));
      expect(threadIdle).toBe(false);
      expect(isThreadQuiescent(map.get(id)!.meta.status)).toBe(false);

      const status = exchangeStatus(boundary, '', true, false,  true, threadIdle);
      expect(isActive(status)).toBe(false);
      expect(status).toBe('aborted');
    });
  }

  it('has nothing renderable to show under the boundary', () => {
    resetSeqCounter();
    const { exchanges } = seedTeardownBoundary('cc-teardown-empty');
    const boundary = exchanges[exchanges.length - 1];

    const events = exchangeResponseEvents(boundary, true, false);
    expect(events.length).toBeGreaterThan(0);
    expect(hasRenderableResponseContent(events)).toBe(false);
  });

  it('still reads Working for a live turn under a non-switch boundary', () => {
    resetSeqCounter();
    const { map, id } = makeThread('safety-net-live', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'close out the pipeline', created: t(-300000) },
      { type: 'ToolCalled', name: 'run_bash', args: { command: 'ls' }, created: t(-299000) },
      { type: 'ResponseAborted', cause: 'safety_net', actor: { kind: 'system' }, created: t(-290000) },
      { type: 'ThoughtStreamed', text: 'Context: 63064 tokens', created: t(-280000) },
      { type: 'ToolCalled', name: 'edit_file', args: { path: 'a.md' }, created: t(-279000) },
      { type: 'ToolResult', name: 'edit_file', result: 'ok', created: t(-278000) },
      { type: 'TextStreamed', text: 'Cut and carried over.', created: t(-277000) },
    ] as any);

    expect(map.get(id)!.meta.status).toBe('failed');
    const live = getExchanges(map, id).slice(-1)[0];
    expect(live.userEvent.type).toBe('ResponseAborted');
    expect(exchangeStatus(live, '', true, false,  false, false)).toBe('streaming');
    expect(hasRenderableResponseContent(exchangeResponseEvents(live, true, false))).toBe(true);

    insertEvents(map, id, [{ type: 'ResponseGenerated', text: 'done', created: t(-276000) }] as any);
    const finished = getExchanges(map, id).slice(-1)[0];
    expect(exchangeStatus(finished, '', true, false, false, false)).toBe('done');
  });
});

describe('an UNATTRIBUTED engine shutdown is just as down as a switch', () => {
  const now = Date.now();
  const t = (offset: number) => new Date(now + offset).toISOString();

  const seedUnattributedTeardown = (name: string, esc = false) => {
    const { map, id } = makeThread(name, 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'pin the copy against reintroduction', channel: 'claude_code', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      { type: 'CodingAgentToolCalled', name: 'Write', args: { file_path: 'guard.test.ts' }, created: t(-290000) },
      { type: 'CodingAgentToolResult', name: 'Write', result: 'BLOCKED: no em dashes', created: t(-289000) },
      { type: 'ResponseAborted', cause: 'engine_shutdown', created: t(-200000) },
      ...(esc
        ? [{ type: 'CodingAgentToolResult' as const, name: '', result: "The user doesn't want to proceed with this tool use.", created: t(-199959) }]
        : []),
      { type: 'CodingAgentTextStreamed', text: '\n\n', created: t(-199948) },
    ] as any);
    return { map, id, exchanges: getExchanges(map, id) };
  };

  for (const [label, esc] of [['a bare flush', false], ['an Esc rejection too', true]] as const) {
    it(`reads as settled, not Working, with ${label}`, () => {
      resetSeqCounter();
      const { map, id, exchanges } = seedUnattributedTeardown(`cc-unattributed-${esc}`, esc);

      const boundary = exchanges[exchanges.length - 1];
      expect(boundary.userEvent.type).toBe('ResponseAborted');
      expect(boundary.steps.length).toBeGreaterThan(0);

      expect(map.get(id)!.meta.status).toBe('failed');
      const threadIdle = isRenderedThreadIdle(map.get(id));
      expect(threadIdle).toBe(false);
      expect(isThreadQuiescent(map.get(id)!.meta.status)).toBe(false);

      const status = exchangeStatus(boundary, '', true, false,  true, threadIdle);
      expect(isActive(status)).toBe(false);
      expect(status).toBe('aborted');
    });
  }

  it('derives no live Thinking row over the dead subprocess', () => {
    resetSeqCounter();
    const { exchanges } = seedUnattributedTeardown('cc-unattributed-thinking');
    const boundary = exchanges[exchanges.length - 1];

    const pendingThinking = (s: { description?: string; outcome: StepOutcome }) =>
      s.description === 'Thinking' && s.outcome === 'pending';

    expect(exchangeSteps(boundary, true, false).filter(pendingThinking)).toHaveLength(0);
    const events = exchangeResponseEvents(boundary, true, false);
    expect(events.filter(e => e.type === 'step' && pendingThinking(e))).toHaveLength(0);

    expect(events.length).toBeGreaterThan(0);
    expect(hasRenderableResponseContent(events)).toBe(false);
  });

  it('still offers Continue and still says nothing about resuming', () => {
    resetSeqCounter();
    const { exchanges } = seedUnattributedTeardown('cc-unattributed-continue');
    const boundary = exchanges[exchanges.length - 1];
    const abort = boundary.userEvent as Extract<ThreadEvent, { type: 'ResponseAborted' }>;

    expect(abortTookEngineDown(abort)).toBe(true);
    expect(abortPromisesAutoResume(abort)).toBe(false);
    expect(responseAbortedSummary(abort.actor, abort.cause)).toBe('Response interrupted');
    expect(continuableAbortIndex(exchanges)).toBe(exchanges.length - 1);
  });
});
