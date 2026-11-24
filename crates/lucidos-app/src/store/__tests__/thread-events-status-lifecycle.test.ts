import { describe, it, expect } from 'vitest';
import { TS, buildCCThread, makeThreadState } from './thread-events-helpers';
import { applySummaryVersion, computeExchanges, exchangeResponseEvents, exchangeStatus, exchangeSteps, groupIntoExchanges, handleEvent, isExchangeStartEvent, synthesizeContextCapture, type StoredEvent, type ThreadEvent } from '../thread-events';
import { handleEventWithAgg } from './aggregate-test-helper';

describe('exchangeStatus — CC follow-up in-progress states', () => {
  it('follow-up with no response events yet = pending', () => {
    const { statuses, exchanges } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
    ]);
    expect(exchanges).toHaveLength(2);
    expect(statuses[0]).toBe('done');
    expect(statuses[1]).toBe('pending');
  });

  it('follow-up with CC tool calls in progress = coding-agent-working', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
    ]);
    expect(statuses[1]).toBe('coding-agent-working');
  });

  it('follow-up mid-streaming (text arrived, no completion) = coding-agent-working', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentTextStreamed', text: 'Working on it...' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
    ]);
    expect(statuses[1]).toBe('coding-agent-working');
  });
});

describe('exchangeStatus — CC follow-up edge cases', () => {
  it('ResponseAborted on exchange 1 does NOT infect exchange 2', () => {
    const { statuses, exchanges } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'first', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'try again', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'SessionStarted', session_id: 's2' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 6, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
    ]);
    expect(exchanges).toHaveLength(3);
    expect(statuses[0]).toBe('aborted');
    expect(exchanges[1].userEvent.type).toBe('ResponseAborted');
    expect(statuses[2]).toBe('done');
  });

  it('CC resumes after idle then completes = done (idle → work → idle)', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      { seq: 5, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:04Z' },
      { seq: 6, event: { type: 'CodingAgentPromptSent', text: 'Run /harden now.' } as ThreadEvent, created: '2026-04-12T10:00:05Z' },
      { seq: 7, event: { type: 'CodingAgentToolCalled', name: 'Bash', args: {} } as ThreadEvent, created: '2026-04-12T10:00:06Z' },
      { seq: 8, event: { type: 'CodingAgentToolResult', name: 'Bash', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:00:07Z' },
      { seq: 9, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:00:08Z' },
    ]);
    expect(statuses).toHaveLength(1);
    expect(statuses[0]).toBe('done');
  });

  it('follow-up sent while CC is actively working (not idle) = coding-agent-working for exchange 1', () => {
    const { statuses, exchanges } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'actually do this instead', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      { seq: 5, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:00:04Z' },
      { seq: 6, event: { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:00:05Z' },
      { seq: 7, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:06Z' },
    ]);
    expect(exchanges).toHaveLength(2);
    expect(statuses[0]).toBe('interrupted');
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

  it('mid-flight cancel during active CC: follow-up exchange shows canceled (not done) after engine_restart_interrupt idle', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'fix', channel: 'claude_code', _eventId: 'A', created: '2026-04-12T10:00:00Z' }],
      [2, { type: 'SessionStarted', session_id: 's1', request_event_id: 'A', created: '2026-04-12T10:00:01Z' } as StoredEvent],
      [3, { type: 'CodingAgentToolCalled', name: 'Read', args: {}, request_event_id: 'A', created: '2026-04-12T10:00:02Z' } as StoredEvent],
      [4, { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code', _eventId: 'B', created: '2026-04-12T10:00:03Z' }],
      [5, { type: 'CodingAgentTextStreamed', text: 'continuing', request_event_id: 'A', created: '2026-04-12T10:00:04Z' } as StoredEvent],
      [6, { type: 'CodingAgentPromptSent', text: 'follow-up', request_event_id: 'A', created: '2026-04-12T10:00:05Z' } as StoredEvent],
      [7, { type: 'ResponseCanceled', channel: 'claude_code', request_event_id: 'A', created: '2026-04-12T10:00:06Z' } as StoredEvent],
      [8, { type: 'CodingAgentIdled', reason: 'engine_restart_interrupt', created: '2026-04-12T10:30:00Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(3);
    const status = exchangeStatus(exchanges[1], '', false, false, true);
    expect(status).toBe('canceled');
    expect(exchanges[2].userEvent.type).toBe('ResponseCanceled');
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

    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, '2026-04-12T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-12T10:00:01Z');
    handleEvent(map, 't', 3, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, '2026-04-12T10:00:02Z');

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
    expect(status1).toBe('pending');
  });

  it('optimistic follow-up resolved by SSE MessageReceived = normal flow', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t', thread]]);

    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, '2026-04-12T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-12T10:00:01Z');
    handleEvent(map, 't', 3, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, '2026-04-12T10:00:02Z');

    thread.pendingUserMessages.push({
      text: 'follow-up',
      eventId: 'msg-1',
      created: '2026-04-12T10:01:00Z',
    });

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

describe('exchangeStatus — Claude Code session recovery and restart', () => {
  it('ContinuationStarted exchange after restart = done (not aborted)', () => {
    const { statuses, exchanges } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'SessionEnded', reason: 'shutdown' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      { seq: 5, event: { type: 'ContinuationStarted', branch: 'claude-code/fix' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 6, event: { type: 'SessionStarted', session_id: 's2' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 7, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
    ]);
    expect(exchanges).toHaveLength(2);
    expect(statuses[0]).toBe('aborted');
    expect(statuses[1]).toBe('done');
  });

});

describe('exchangeStatus — CC follow-up grouping correctness', () => {
  it('CodingAgentUserMessageSent dedupes with preceding MessageReceived', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix', channel: 'claude_code', created: '2026-04-12T10:00:00Z' } as ThreadEvent],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-04-12T10:00:01Z' } as ThreadEvent],
      [3, { type: 'CodingAgentIdled', has_changes: false, created: '2026-04-12T10:00:02Z' } as ThreadEvent],
      [4, { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code', created: '2026-04-12T10:01:00Z' } as ThreadEvent],
      [5, { type: 'CodingAgentUserMessageSent', text: 'follow-up', created: '2026-04-12T10:01:00Z' } as ThreadEvent],
      [6, { type: 'CodingAgentIdled', has_changes: false, created: '2026-04-12T10:01:01Z' } as ThreadEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('MessageReceived');
  });

  it('legacy CodingAgentUserMessageSent without MessageReceived creates exchange', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'fix', channel: 'claude_code', created: '2026-04-12T10:00:00Z' } as ThreadEvent],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-04-12T10:00:01Z' } as ThreadEvent],
      [3, { type: 'CodingAgentIdled', has_changes: false, created: '2026-04-12T10:00:02Z' } as ThreadEvent],
      [4, { type: 'CodingAgentUserMessageSent', text: 'follow-up', created: '2026-04-12T10:01:00Z' } as ThreadEvent],
      [5, { type: 'CodingAgentIdled', has_changes: false, created: '2026-04-12T10:01:01Z' } as ThreadEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
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
      { seq: 6, event: { type: 'MessageReceived', text: 'second', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 7, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 8, event: { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
      { seq: 9, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:01:03Z' },
    ]);
    expect(exchanges[0].steps).toHaveLength(4);
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
    const { statuses, exchanges } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'now fix tests', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentUserMessageSent', text: 'now fix tests' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 6, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 7, event: { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
      { seq: 8, event: { type: 'CodingAgentTextStreamed', text: 'Tests fixed!' } as ThreadEvent, created: '2026-04-12T10:01:03Z' },
      { seq: 9, event: { type: 'ResponseGenerated', text: 'Tests fixed!' } as ThreadEvent, created: '2026-04-12T10:01:04Z' },
      { seq: 10, event: { type: 'CodingAgentPromptSent', text: 'Run /harden now.' } as ThreadEvent, created: '2026-04-12T10:01:05Z' },
      { seq: 11, event: { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'cargo test' } } as ThreadEvent, created: '2026-04-12T10:01:06Z' },
      { seq: 12, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:01:07Z' },
    ]);
    expect(exchanges).toHaveLength(3);
    expect(statuses[0]).toBe('done');
    expect(statuses[1]).toBe('done');
    expect(exchanges[2].userEvent.type).toBe('ResponseAborted');
  });

  it('follow-up completed (CodingAgentIdled) then auto-harden crash = done, NOT aborted', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentTextStreamed', text: 'Done!' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 6, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
      { seq: 7, event: { type: 'CodingAgentPromptSent', text: 'Run /harden now.' } as ThreadEvent, created: '2026-04-12T10:01:03Z' },
      { seq: 8, event: { type: 'CodingAgentToolCalled', name: 'Bash', args: {} } as ThreadEvent, created: '2026-04-12T10:01:04Z' },
      { seq: 9, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:01:05Z' },
    ]);
    expect(statuses[1]).toBe('done');
  });

  it('initial exchange completed then system prompt crash = done, NOT aborted', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      { seq: 5, event: { type: 'ResponseGenerated', text: 'Fixed!' } as ThreadEvent, created: '2026-04-12T10:00:04Z' },
      { seq: 6, event: { type: 'CodingAgentPromptSent', text: 'Run /harden now.' } as ThreadEvent, created: '2026-04-12T10:00:05Z' },
      { seq: 7, event: { type: 'CodingAgentToolCalled', name: 'Bash', args: {} } as ThreadEvent, created: '2026-04-12T10:00:06Z' },
      { seq: 8, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:00:07Z' },
    ]);
    expect(statuses[0]).toBe('done');
  });

  it('genuine crash before any completion = aborted (not affected by fix)', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
    ]);
    expect(statuses[0]).toBe('aborted');
  });

  it('CC crash during follow-up before any completion = aborted (genuine crash)', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 6, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
    ]);
    expect(statuses[1]).toBe('aborted');
  });

  it('shutdown after CodingAgentIdled: exchange was complete = done, NOT aborted', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      { seq: 5, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:00:04Z' },
      { seq: 6, event: { type: 'SessionEnded', reason: 'shutdown' } as ThreadEvent, created: '2026-04-12T10:00:05Z' },
    ]);
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

describe('Phase 4 — thread lifecycle under terminal-only SessionEnded', () => {
  it('thread is active after CodingAgentIdled', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'first turn' } as ThreadEvent, '2026-04-24T10:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-24T10:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, '2026-04-24T10:00:02Z');
    handleEventWithAgg(map, 't1', 4, { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, '2026-04-24T10:00:03Z');

    expect(thread.meta.status).toBe('idle');
    expect(map.has('t1')).toBe(true);

    handleEventWithAgg(map, 't1', 5, { type: 'MessageReceived', text: 'second turn' } as ThreadEvent, '2026-04-24T10:00:10Z');
    expect(thread.meta.status).toBe('running');
  });

  it('thread is active after ChangeProposed', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    applySummaryVersion(thread.meta, thread.meta.summaryVersion, 'running');
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'do work' } as ThreadEvent, '2026-04-24T10:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-24T10:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'ChangeProposed', change_id: 'c1', commit_sha: 'aaa111', description: 'first' } as ThreadEvent, '2026-04-24T10:00:02Z');
    handleEventWithAgg(map, 't1', 4, { type: 'ChangeProposed', change_id: 'c1', commit_sha: 'bbb222', description: 'second' } as ThreadEvent, '2026-04-24T10:00:03Z');
    handleEventWithAgg(map, 't1', 5, { type: 'ChangeProposed', change_id: 'c1', commit_sha: 'ccc333', description: 'third' } as ThreadEvent, '2026-04-24T10:00:04Z');

    expect(thread.meta.status).toBe('running');
    expect(thread.meta.codingAgentChangeState.kind).toBe('proposed');
    const proposed = [...thread.events.values()].filter(e => e.type === 'ChangeProposed');
    expect(proposed).toHaveLength(3);
    expect(proposed.map(e => (e as { commit_sha?: string }).commit_sha)).toEqual(['aaa111', 'bbb222', 'ccc333']);
    expect(map.has('t1')).toBe(true);
  });

  it('thread is active after ResponseCanceled', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'long task' } as ThreadEvent, '2026-04-24T10:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-24T10:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} } as ThreadEvent, '2026-04-24T10:00:02Z');
    handleEventWithAgg(map, 't1', 4, { type: 'ResponseCanceled' } as ThreadEvent, '2026-04-24T10:00:03Z');

    expect(thread.meta.status).toBe('idle');
    expect(map.has('t1')).toBe(true);

    handleEventWithAgg(map, 't1', 5, { type: 'MessageReceived', text: 'try again' } as ThreadEvent, '2026-04-24T10:00:10Z');
    expect(thread.meta.status).toBe('running');
  });

  it('thread is closed only on SessionEnded with terminal reason', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t1', thread]]);

    handleEventWithAgg(map, 't1', 1, { type: 'MessageReceived', text: 'do work' } as ThreadEvent, '2026-04-24T10:00:00Z');
    handleEventWithAgg(map, 't1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-24T10:00:01Z');
    handleEventWithAgg(map, 't1', 3, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, '2026-04-24T10:00:02Z');
    handleEventWithAgg(map, 't1', 4, { type: 'SessionEnded', reason: 'shutdown' } as ThreadEvent, '2026-04-24T10:00:03Z');

    const exchanges = groupIntoExchanges(thread.events);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true, false, true)).toBe('aborted');
  });
});

describe('ContextCaptured projection — main_llm vs claude_code', () => {
  it('a main_llm snapshot survives the tool call that names its row', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'hi', created: '2026-05-12T10:00:00Z' } as ThreadEvent],
      [2, { type: 'ThoughtStreamed', text: 'Context: 100 tokens, 1 messages' } as ThreadEvent],
      [3, {
        type: 'ContextCaptured',
        producer: 'main_llm',
        model: 'claude-opus-4-7',
        context_window: 200_000,
        sections: [],
        tools: [],
        estimated_total_tokens: 100,
        usage: { input_tokens: 100, output_tokens: 0, cache_read_tokens: 0, cache_creation_tokens: 0 },
      } as ThreadEvent],
      [4, { type: 'ToolCalled', name: 'read_file', args: {} } as ThreadEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const respSteps = exchangeResponseEvents(exchanges[0]).filter(e => e.type === 'step') as Array<{ description: string; contextCapture?: { producer: string } }>;
    expect(respSteps).toHaveLength(1);
    expect(respSteps[0].description).not.toBe('Thinking');
    expect(respSteps[0].contextCapture?.producer).toBe('main_llm');
  });

  describe('a held-back prose-nudge round', () => {
    const capture = (input_tokens: number) => ({
      type: 'ContextCaptured',
      producer: 'main_llm',
      model: 'claude-opus-4-7',
      context_window: 200_000,
      sections: [],
      tools: [],
      estimated_total_tokens: input_tokens,
      usage: { input_tokens, output_tokens: 0, cache_read_tokens: 0, cache_creation_tokens: 0 },
    } as ThreadEvent);
    type Row = { description: string; contextCapture?: { usage?: { input_tokens: number } } };
    const rowsOf = (events: Map<number, ThreadEvent>): Record<string, Row[]> => {
      const exchange = groupIntoExchanges(events)[0];
      return {
        exchangeSteps: exchangeSteps(exchange) as Row[],
        exchangeResponseEvents: exchangeResponseEvents(exchange).filter(e => e.type === 'step') as Row[],
      };
    };
    const draftRound: Array<[number, ThreadEvent]> = [
      [1, { type: 'MessageReceived', text: 'hey!', created: '2026-09-30T06:30:30Z' } as ThreadEvent],
      [2, { type: 'ThoughtStreamed', text: 'Context: 100 tokens, 1 messages' } as ThreadEvent],
      [3, capture(100)],
      [4, { type: 'TextStreamed', text: 'What are we working on this morning?' } as ThreadEvent],
    ];

    it('declined: the draft row keeps its own snapshot', () => {
      const events = new Map<number, ThreadEvent>([
        ...draftRound,
        [5, capture(200)],
        [6, { type: 'ResponseGenerated', text: 'What are we working on this morning?' } as ThreadEvent],
      ]);
      for (const [projection, rows] of Object.entries(rowsOf(events))) {
        expect(rows, projection).toHaveLength(1);
        expect(rows[0].contextCapture?.usage?.input_tokens, projection).toBe(100);
      }
    });

    it('calling a tool: the round gets its own row and snapshot', () => {
      const events = new Map<number, ThreadEvent>([
        ...draftRound,
        [5, { type: 'ThoughtStreamed', text: 'Context: 200 tokens, 3 messages' } as ThreadEvent],
        [6, capture(200)],
        [7, { type: 'ToolCalled', name: 'ask_user_question', args: {}, description: 'Asking: which one?' } as ThreadEvent],
      ]);
      for (const [projection, rows] of Object.entries(rowsOf(events))) {
        expect(rows.map(r => r.description), projection).toEqual(['Thinking', 'Asking: which one?']);
        expect(rows.map(r => r.contextCapture?.usage?.input_tokens), projection).toEqual([100, 200]);
      }
    });
  });

  it('claude_code snapshot binds to the most recent step (any kind)', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'do work', created: '2026-05-12T10:00:00Z' } as ThreadEvent],
      [2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent],
      [3, { type: 'CodingAgentPromptSent' } as ThreadEvent],
      [4, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'a.rs' }, tool_use_id: 'tu-A' } as ThreadEvent],
      [5, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', tool_use_id: 'tu-A' } as ThreadEvent],
      [6, {
        type: 'ContextCaptured',
        producer: 'claude_code',
        model: 'claude-opus-4-7',
        context_window: 200_000,
        sections: [],
        tools: [],
        estimated_total_tokens: 5000,
        usage: { input_tokens: 5000, output_tokens: 100, cache_read_tokens: 0, cache_creation_tokens: 0 },
      } as ThreadEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const respSteps = exchangeResponseEvents(exchanges[0]).filter(e => e.type === 'step') as Array<{ description: string; contextCapture?: { producer: string; usage?: { input_tokens: number } } }>;
    const readStep = respSteps.find(s => s.description === 'Read a.rs')!;
    expect(readStep.contextCapture?.producer).toBe('claude_code');
    expect(readStep.contextCapture?.usage?.input_tokens).toBe(5000);
  });
});

describe('synthesizeContextCapture — legacy event projection', () => {
  it('synthesizeContextCapture stitches legacy Thinking + ContextTokensMeasured + ContextAssembled', () => {
    const result = synthesizeContextCapture({
      thinking: { text: 'Context: 36510 tokens, 1 messages', context_tokens: 36510, context_messages: 1 },
      tokensMeasured: { input_tokens: 23500 },
      assembled: { sections: [{ name: 'System', content: 'sys', budget_delta_chars: 3 }], tools: ['read_file'], model: 'claude-opus-4-7' },
    });
    expect(result.producer).toBe('main_llm');
    expect(result.model).toBe('claude-opus-4-7');
    expect(result.sections).toHaveLength(1);
    expect(result.usage?.input_tokens).toBe(23500);
    expect(result.estimated_total_tokens).toBe(36510);
    expect(result.legacy).toBe(true);
  });

  it('synthesizeContextCapture survives missing tokensMeasured', () => {
    const result = synthesizeContextCapture({
      thinking: { text: 'Context: 1000 tokens, 2 messages', context_tokens: 1000 },
      assembled: { sections: [], tools: [], model: 'claude-sonnet-4-6' },
    });
    expect(result.usage).toBeUndefined();
    expect(result.estimated_total_tokens).toBe(1000);
    expect(result.model).toBe('claude-sonnet-4-6');
    expect(result.legacy).toBe(true);
  });

  it('synthesizeContextCapture reports no tokens rather than a char count', () => {
    const result = synthesizeContextCapture({
      assembled: {
        sections: [{ name: 'System', budget_delta_chars: 147_800 }],
        tools: [],
        model: 'claude-opus-4-7',
      },
    });
    expect(result.estimated_total_tokens).toBe(0);
    expect(result.usage).toBeUndefined();
    expect(result.sections[0].budget_delta_chars).toBe(147_800);
  });

  it('synthesizeContextCapture survives missing assembled', () => {
    const result = synthesizeContextCapture({
      thinking: { text: 'Context: 500 tokens, 1 messages', context_tokens: 500, context_messages: 1 },
    });
    expect(result.sections).toEqual([]);
    expect(result.tools).toEqual([]);
    expect(result.model).toBe('');
    expect(result.estimated_total_tokens).toBe(500);
    expect(result.legacy).toBe(true);
  });
});

describe('ChildThreadCompleted as exchange-starter', () => {
  it('is recognized as an exchange-starter', () => {
    expect(isExchangeStartEvent({ type: 'ChildThreadCompleted' })).toBe(true);
  });
});

describe('handleEvent projects TodoListWritten into meta.latestTodoList', () => {
  it('leaves meta.latestTodoList null when no TodoListWritten arrived', () => {
    const map = new Map([['thread-1', makeThreadState()]]);
    handleEvent(map, 'thread-1', 1, { type: 'TextStreamed', text: 'hi' }, TS);
    handleEvent(map, 'thread-1', 2, { type: 'ToolCalled', name: 'read_file', args: { path: 'x' } }, TS);
    expect(map.get('thread-1')!.meta.latestTodoList).toBeNull();
  });

  it('sets meta.latestTodoList from the single TodoListWritten event', () => {
    const map = new Map([['thread-1', makeThreadState()]]);
    handleEvent(map, 'thread-1', 1, { type: 'TextStreamed', text: 'starting' }, TS);
    handleEvent(map, 'thread-1', 2, { type: 'TodoListWritten', items: [
      { content: 'Run tests', active_form: 'Running tests', status: 'pending' },
      { content: 'Update docs', active_form: 'Updating docs', status: 'pending' },
    ] }, TS);
    expect(map.get('thread-1')!.meta.latestTodoList).toEqual([
      { content: 'Run tests', active_form: 'Running tests', status: 'pending' },
      { content: 'Update docs', active_form: 'Updating docs', status: 'pending' },
    ]);
  });

  it('overwrites meta.latestTodoList with the most recent TodoListWritten', () => {
    const map = new Map([['thread-1', makeThreadState()]]);
    handleEvent(map, 'thread-1', 1, { type: 'TodoListWritten', items: [
      { content: 'a', active_form: 'doing a', status: 'pending' },
    ] }, TS);
    handleEvent(map, 'thread-1', 2, { type: 'ToolCalled', name: 'read_file', args: {} }, TS);
    handleEvent(map, 'thread-1', 3, { type: 'TodoListWritten', items: [
      { content: 'a', active_form: 'doing a', status: 'completed' },
      { content: 'b', active_form: 'doing b', status: 'in_progress' },
    ] }, TS);
    handleEvent(map, 'thread-1', 4, { type: 'TextStreamed', text: 'mid' }, TS);
    expect(map.get('thread-1')!.meta.latestTodoList).toEqual([
      { content: 'a', active_form: 'doing a', status: 'completed' },
      { content: 'b', active_form: 'doing b', status: 'in_progress' },
    ]);
  });

  it('sets meta.latestTodoList to [] when the agent clears the list', () => {
    const map = new Map([['thread-1', makeThreadState()]]);
    handleEvent(map, 'thread-1', 1, { type: 'TodoListWritten', items: [
      { content: 'a', active_form: 'doing a', status: 'completed' },
    ] }, TS);
    handleEvent(map, 'thread-1', 2, { type: 'TodoListWritten', items: [] }, TS);
    expect(map.get('thread-1')!.meta.latestTodoList).toEqual([]);
  });
});
