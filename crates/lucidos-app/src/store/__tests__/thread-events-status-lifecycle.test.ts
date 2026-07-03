import { describe, it, expect } from 'vitest';
import { TS, buildCCThread, makeThreadState } from './thread-events-helpers';
import { computeExchanges, exchangeResponseEvents, exchangeStatus, groupIntoExchanges, handleEvent, isExchangeStartEvent, synthesizeContextCapture, type StoredEvent, type ThreadEvent } from '../thread-events';
import { handleEventWithAgg } from './aggregate-test-helper';

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

  it('follow-up with CC tool calls in progress = coding-agent-working', () => {
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Follow-up — CC starts working
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

  it('follow-up sent while CC is actively working (not idle) = coding-agent-working for exchange 1', () => {
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
    // 1: A's CC reply, 2: B follow-up (canceled), 3: cancel boundary panel
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

describe('exchangeStatus — Claude Code session recovery and restart', () => {
  it('ContinuationStarted exchange after restart = done (not aborted)', () => {
    const { statuses, exchanges } = buildCCThread([
      // Exchange 1: original session, engine restarted
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'SessionEnded', reason: 'shutdown' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      // Exchange 2: recovery after restart
      { seq: 5, event: { type: 'ContinuationStarted', branch: 'claude-code/fix' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
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
    // The status rule is 'no_change' — only codingAgentProposed flips on. Multiple
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
    // codingAgentProposed flips on, all three events are preserved (no overwrites).
    expect(thread.meta.status).toBe('running');
    expect(thread.meta.codingAgentProposed).toBe(true);
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
    // user's Claude Code session was actually killed by the engine — every other
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

describe('ContextCaptured projection — main_llm vs claude_code', () => {
  // Main-LLM snapshots fire after a Thinking step — bind there so the
  // inline chip renders next to the request. CC has no per-API-call
  // Thinking step (CC manages its own loop), so a CC snapshot must bind
  // to whatever step is on top of the stack at emission time —
  // typically the tool that just finished. Without this split, every CC
  // step's modal would still show "No context snapshot captured."
  it('main_llm snapshot binds to the most recent Thinking step', () => {
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
    const thinking = respSteps.find(s => s.description === 'Thinking');
    const tool = respSteps.find(s => s.description !== 'Thinking');
    expect(thinking?.contextCapture?.producer).toBe('main_llm');
    expect(tool?.contextCapture).toBeUndefined();
  });

  it('claude_code snapshot binds to the most recent step (any kind)', () => {
    // CC emits one ContextCaptured per LLM API call (see
    // run_session.rs::AgentEvent::Usage). Between calls, CC may have
    // executed several tools — the snapshot binds to the latest CC step
    // so the user sees real token usage when they click any of them.
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
    // Find the Read step (the most recent step before the ContextCaptured event)
    const lastStep = respSteps[respSteps.length - 1];
    expect(lastStep.contextCapture?.producer).toBe('claude_code');
    expect(lastStep.contextCapture?.usage?.input_tokens).toBe(5000);
  });
});

describe('synthesizeContextCapture — legacy event projection', () => {
  // The new ContextCaptured event replaces the legacy
  // Thinking{tokens} + ContextTokensMeasured + ContextAssembled trio. Old
  // DB rows still surface as those three events; the shim stitches whatever
  // subset is present into a ContextCapture so the same modal renders
  // for replays without a backend migration. `legacy: true` lets the modal
  // badge it so the user knows cache stats / section bodies may be partial.
  it('synthesizeContextCapture stitches legacy Thinking + ContextTokensMeasured + ContextAssembled', () => {
    const result = synthesizeContextCapture({
      thinking: { text: 'Context: 36510 tokens, 1 messages', context_tokens: 36510, context_messages: 1 },
      tokensMeasured: { input_tokens: 23500 },
      assembled: { sections: [{ name: 'System', content: 'sys', char_count: 3 }], tools: ['read_file'], model: 'claude-opus-4-7', total_chars: 3 },
    });
    expect(result.producer).toBe('main_llm');
    expect(result.model).toBe('claude-opus-4-7');
    expect(result.sections).toHaveLength(1);
    expect(result.usage?.input_tokens).toBe(23500);
    expect(result.estimated_total_tokens).toBe(36510);
    expect(result.legacy).toBe(true);
  });

  // Pre-ContextTokensMeasured (very old rows): only Thinking + ContextAssembled
  // present. usage stays undefined; the modal still renders the section
  // breakdown, just without a real cache hit rate.
  it('synthesizeContextCapture survives missing tokensMeasured', () => {
    const result = synthesizeContextCapture({
      thinking: { text: 'Context: 1000 tokens, 2 messages', context_tokens: 1000 },
      assembled: { sections: [], tools: [], model: 'claude-sonnet-4-6', total_chars: 0 },
    });
    expect(result.usage).toBeUndefined();
    expect(result.estimated_total_tokens).toBe(1000);
    expect(result.model).toBe('claude-sonnet-4-6');
    expect(result.legacy).toBe(true);
  });

  // Even-older rows where only Thinking ever fired (capture_context off).
  // No section list, no model — model defaults to empty string so the modal
  // can render "(unknown model)". Producer stays main_llm; legacy=true.
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
    expect(isExchangeStartEvent('ChildThreadCompleted')).toBe(true);
  });
});

// `handleEvent` projects `TodoListWritten` items into `meta.latestTodoList`
// so the prompt-bar indicator reads O(1) per render instead of walking the
// events Map. Replace-whole-list semantics: latest call wins; `[]` is a valid
// "cleared" state; `null` means the agent never wrote one.
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
