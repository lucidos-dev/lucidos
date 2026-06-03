import { describe, it, expect, beforeEach } from 'vitest';
import { TS, getExchanges, getLabel, insertEvents, makeThread, nextSeq, resetSeqCounter } from './thread-flows-helpers';
import { exchangeResponseEvents, exchangeResponseTimestamp, exchangeStatus, exchangeTimestamp, exchangeUserChannel, exchangeUserMessage, groupIntoExchanges, handleEvent, type Exchange, type ThreadState } from '../thread-events';
import { handleEventWithAgg } from './aggregate-test-helper';
import { isActive, statusLabel } from '../exchange-status';
import { effectiveThreadStatus } from '../store';

beforeEach(resetSeqCounter);

describe('Timestamps', () => {
  it('response timestamp differs from user timestamp', () => {
    const { map, id } = makeThread();
    const userTime = '2026-03-15T20:54:09.000Z';
    const responseTime = '2026-03-15T20:54:12.000Z';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Hello', created: userTime } as any,
      { type: 'TextStreamed', text: 'Hi there!', created: responseTime } as any,
      { type: 'ResponseGenerated', created: responseTime } as any,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeTimestamp(exchanges[0])).toBe(userTime);
    expect(exchangeResponseTimestamp(exchanges[0])).toBe(responseTime);
    // They must be different
    expect(exchangeTimestamp(exchanges[0])).not.toBe(exchangeResponseTimestamp(exchanges[0]));
  });

  it('handleEvent stores server-provided created timestamp, not client time', () => {
    const { map, id } = makeThread();
    const serverTime = '2026-03-15T20:54:09.000Z';

    handleEvent(map, id, 1, { type: 'MessageReceived', text: 'Hello' }, serverTime);

    const thread = map.get(id)!;
    const stored = thread.events.get(1)!;
    expect(stored.created).toBe(serverTime);
  });

  it('non-last chat exchange with steps shows interrupted when follow-up arrives', () => {
    const { map, id } = makeThread();

    // First exchange is still processing (no ResponseGenerated)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'First message', created: '2026-01-01T00:00:00Z' },
      { type: 'ToolCalled', name: 'search', args: {}, created: '2026-01-01T00:00:01Z' },
    ]);

    // Second exchange is pending (user sent follow-up while first is processing)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Follow-up', created: '2026-01-01T00:00:02Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    // First exchange (non-last, has steps, no terminator) → 'interrupted'.
    // The user moved on with the follow-up; the chat fast-path will fold the
    // follow-up into the running loop via UPI, with post-UPI events redirected
    // to the new exchange. Only the last panel shows "Working".
    const firstStatus = exchangeStatus(exchanges[0], '', false);
    expect(firstStatus).toBe('interrupted');

    // Second exchange (last, no steps yet). hasPriorActive is false because
    // the prior is now 'interrupted' (not in ACTIVE_STATUSES) — the follow-up
    // is no longer "queued behind" the prior; it's the new active panel.
    const secondStatus = exchangeStatus(exchanges[1], '', true, false);
    expect(secondStatus).toBe('pending');
  });

  it('pending exchange is not queued when no prior active exchange', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Only message' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true, false)).toBe('pending');
  });

  it('queued exchange becomes pending once prior exchange completes', () => {
    const { map, id } = makeThread();
    const now = new Date();
    const t = (offsetMs: number) => new Date(now.getTime() + offsetMs).toISOString();

    // First exchange completes normally
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'First', created: t(0) },
      { type: 'TextStreamed', text: 'Response', created: t(100) },
      { type: 'ResponseGenerated', created: t(200) },
    ]);

    // Second exchange is pending (prior finished)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Follow-up', created: t(300) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    // First exchange is done (completed)
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');

    // Second exchange: prior is NOT active (done), so hasPriorActive=false → pending, not queued
    expect(exchangeStatus(exchanges[1], '', true, false)).toBe('pending');
  });

  it('queued check only applies to exchanges with no steps', () => {
    const { map, id } = makeThread();
    const now = new Date();
    const t = (offsetMs: number) => new Date(now.getTime() + offsetMs).toISOString();

    // First exchange still processing
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'First', created: t(0) },
      { type: 'ToolCalled', name: 'search', args: {}, created: t(100) },
    ]);

    // Second exchange has steps — even with hasPriorActive, it shouldn't be 'queued'
    // because the queued check requires steps.length === 0
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Second', created: t(200) },
      { type: 'ToolCalled', name: 'read_file', args: {}, created: t(300) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    // Second exchange has steps, so hasPriorActive doesn't force it to 'queued'
    const status = exchangeStatus(exchanges[1], '', true, true);
    expect(status).not.toBe('queued');
  });

  it('view-layer priorActive computation detects active prior exchange', () => {
    // This test mirrors how ThreadView/CreateThreadView compute priorActive:
    //   const priorActive = i > 0 && isStatusActive(exchangeStatus(exchanges[i-1], '', ...));
    // The bug: passing isLast=false for the prior exchange causes exchangeStatus
    // to shortcut to 'done' (line: if (isComplete || !isLast) return 'done'),
    // making priorActive always false.
    const { map, id } = makeThread();
    const now = new Date();
    const t = (offsetMs: number) => new Date(now.getTime() + offsetMs).toISOString();

    // First exchange is still processing (no ResponseGenerated)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'First message', created: t(0) },
      { type: 'ToolCalled', name: 'search', args: {}, created: t(100) },
    ]);

    // Second exchange is pending (user sent follow-up while first is processing)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Follow-up', created: t(200) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    // Simulate how the view layer computes priorActive:
    // For exchange i=1, prior is exchange 0.
    // The view must pass isLast=true to get the actual live status, not the display status.
    const priorStatus = exchangeStatus(exchanges[0], '', true);
    expect(isActive(priorStatus)).toBe(true); // prior IS active — it's still streaming

    // With priorActive=true, the second exchange should be 'queued'
    const secondStatus = exchangeStatus(exchanges[1], '', true, isActive(priorStatus));
    expect(secondStatus).toBe('queued');
    expect(getLabel(exchanges[1], '', true, isActive(priorStatus))).toBe('Queued');
  });
});

// ---------------------------------------------------------------------------
// Backend is authoritative about liveness — no timestamp guessing
// ---------------------------------------------------------------------------
describe('Backend-authoritative status: SSE events update meta.status', () => {
  it('Claude Code session with tools in progress → status=running (from MessageReceived event)', () => {
    const { map, id } = makeThread();
    const twoMinutesAgo = new Date(Date.now() - 120_000).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', created: twoMinutesAgo },
      { type: 'SessionStarted', session_id: 's1', created: twoMinutesAgo },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: {}, created: twoMinutesAgo },
    ]);

    // MessageReceived sets meta.status='running', no completion event → still running
    expect(map.get(id)!.meta.status).toBe('running');
  });

  it('regular chat with open request → status=running (from MessageReceived event)', () => {
    const { map, id } = makeThread();
    const twoMinutesAgo = new Date(Date.now() - 120_000).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'hello', created: twoMinutesAgo },
      { type: 'ToolCalled', name: 'calculator', args: {}, created: twoMinutesAgo },
    ]);

    // MessageReceived sets status='running', ToolCalled doesn't change it → running
    expect(map.get(id)!.meta.status).toBe('running');
  });
});

// ---------------------------------------------------------------------------
// Change events open their own initiator-panel exchanges (auditable system actions)
// ---------------------------------------------------------------------------
describe('Change lifecycle events render as initiator panels', () => {
  it('ChangeApplied opens its own exchange', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix it' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix', files: ['a.rs'] },
      { type: 'CodingAgentIdled', has_changes: true },
      { type: 'ChangeApplied', change_id: 'c-1' },
      { type: 'SessionEnded' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('ChangeApplied');
    expect((exchanges[1].userEvent as { change_id?: string }).change_id).toBe('c-1');
  });

  it('ChangeDiscarded opens its own exchange', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'try it' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentTextStreamed', text: 'Tried.' },
      { type: 'ChangeProposed', change_id: 'c-2', description: 'Experiment', files: ['b.rs'] },
      { type: 'CodingAgentIdled', has_changes: true },
      { type: 'ChangeDiscarded', change_id: 'c-2' },
      { type: 'SessionEnded' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('ChangeDiscarded');
  });

  it('ChangeReverted opens its own exchange', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix it' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'ChangeProposed', change_id: 'c-3', description: 'Fix', files: ['c.rs'] },
      { type: 'CodingAgentIdled', has_changes: true },
      { type: 'ChangeApplied', change_id: 'c-3' },
      { type: 'ChangeReverted', change_id: 'c-3' },
      { type: 'SessionEnded' },
    ]);

    const exchanges = getExchanges(map, id);
    // 1: CC, 2: ChangeApplied, 3: ChangeReverted
    expect(exchanges).toHaveLength(3);
    expect(exchanges[1].userEvent.type).toBe('ChangeApplied');
    expect(exchanges[2].userEvent.type).toBe('ChangeReverted');
  });

  it('ChangeApplyFailed opens its own exchange', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix it' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'ChangeProposed', change_id: 'c-4', description: 'Fix', files: ['d.rs'] },
      { type: 'CodingAgentIdled', has_changes: true },
      { type: 'ChangeApplyFailed', change_id: 'c-4', error: 'merge conflict' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('ChangeApplyFailed');
    expect((exchanges[1].userEvent as { error?: string }).error).toBe('merge conflict');
  });

  it('SessionEnded does not start a new exchange', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix it' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'SessionEnded', reason: 'completed' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
  });

  it('MergeConflictDetected opens its own initiator-panel exchange', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix it' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'ChangeProposed', change_id: 'c-5', description: 'Fix', files: ['e.rs'] },
      { type: 'CodingAgentIdled', has_changes: true },
      { type: 'MergeConflictDetected', change_id: 'c-5', files: ['e.rs'] },
      { type: 'CodingAgentTextStreamed', text: 'Resolving...' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('MergeConflictDetected');
    const conflictBody = exchangeUserMessage(exchanges[1]);
    expect(conflictBody).toContain('Merging changes from main');
  });

  it('MergeConflictDetected revives idle thread to running status', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix it' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'ChangeProposed', change_id: 'c-6', description: 'Fix', files: ['f.rs'] },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);
    // After CodingAgentIdled, thread status should be 'waiting'
    expect(map.get(id)!.meta.status).toBe('waiting');

    // MergeConflictDetected sets codingAgentApplying=true but doesn't change status
    insertEvents(map, id, [
      { type: 'MergeConflictDetected', change_id: 'c-6', files: ['f.rs'] },
    ]);
    expect(map.get(id)!.meta.status).toBe('waiting');
    expect(map.get(id)!.meta.codingAgentApplying).toBe(true);
  });

  it('CC resumption after ChangeApplied does not leave trailing Thinking on CC exchange', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix it' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix', files: ['a.rs'] },
      { type: 'CodingAgentIdled', has_changes: true },
      { type: 'ChangeApplied', change_id: 'c-1' },
      // CC resumes to process change notification — its events land on the
      // ChangeApplied exchange (no response panel, so invisible to the user).
      { type: 'CodingAgentPromptSent', text: '' },
      { type: 'CodingAgentIdled' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('ChangeApplied');
    // CC exchange ends cleanly — last event is the response text, not a spinner.
    const ccEvents = exchangeResponseEvents(exchanges[0]);
    const lastCC = ccEvents[ccEvents.length - 1];
    expect(lastCC.type).not.toBe('step');
  });
});

// ---------------------------------------------------------------------------
// Flow: Message queue handling
// ---------------------------------------------------------------------------
describe('Flow: Message queue — multiple pending messages', () => {
  it('supports multiple pending messages as synthetic exchanges', () => {
    const { map, id } = makeThread();

    // First message is being processed (has real events)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'First message', created: '2026-01-01T00:00:00Z' },
      { type: 'TextStreamed', text: 'Thinking...', created: '2026-01-01T00:00:01Z' },
    ]);

    // Two more messages queued (pending, no backend events yet)
    const thread = map.get(id)!;
    thread.pendingUserMessages = [
      { text: 'Second message', eventId: 'msg-2', created: '2026-01-01T00:00:00Z' },
      { text: 'Third message', eventId: 'msg-3', created: '2026-01-01T00:00:00Z' },
    ];

    // Build exchanges: 1 real + 2 synthetic
    const exchanges = groupIntoExchanges(thread.events);
    // Append pending messages as synthetic exchanges (simulating activeExchanges computed)
    for (let i = 0; i < thread.pendingUserMessages.length; i++) {
      exchanges.push({
        userEvent: { type: 'MessageReceived', text: thread.pendingUserMessages[i].text },
        userSeq: -(i + 1),
        steps: [],
      });
    }

    expect(exchanges).toHaveLength(3);
    expect(exchangeUserMessage(exchanges[0])).toBe('First message');
    expect(exchangeUserMessage(exchanges[1])).toBe('Second message');
    expect(exchangeUserMessage(exchanges[2])).toBe('Third message');
  });

  it('last queued exchange shows "Queued" status when prior is active', () => {
    const { map, id } = makeThread();

    // First exchange is active (streaming)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'First', created: '2026-01-01T00:00:00Z' },
      { type: 'TextStreamed', text: 'Working...', created: '2026-01-01T00:00:01Z' },
    ]);

    const exchanges = getExchanges(map, id);
    // One queued synthetic exchange (the last in the thread)
    exchanges.push({
      userEvent: { type: 'MessageReceived', text: 'Second' },
      userSeq: -1,
      steps: [],
    });

    // Second exchange: last, has prior active, no steps → 'queued'
    const status1 = exchangeStatus(exchanges[1], '', true, true);
    expect(status1).toBe('queued');
    expect(statusLabel(status1, false).label).toBe('Queued');
  });

  it('superseded queued exchange shows "Continued below" instead of "Queued"', () => {
    const { map, id } = makeThread();

    // First exchange is active (streaming)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'First', created: '2026-01-01T00:00:00Z' },
      { type: 'TextStreamed', text: 'Working...', created: '2026-01-01T00:00:01Z' },
    ]);

    const exchanges = getExchanges(map, id);
    // Two queued synthetic exchanges
    exchanges.push({
      userEvent: { type: 'MessageReceived', text: 'Second' },
      userSeq: -1,
      steps: [],
    });
    exchanges.push({
      userEvent: { type: 'MessageReceived', text: 'Third' },
      userSeq: -2,
      steps: [],
    });

    // Exchange 1 is NOT last (exchange 2 exists after it) → superseded.
    // Even though hasPriorActive=true, it should NOT be 'queued' because it
    // was superseded by a later message. It should be 'done' so that
    // ChatExchange displays it as "Continued below ↳".
    const status1 = exchangeStatus(exchanges[1], '', false, true);
    expect(status1).not.toBe('queued');
    expect(status1).toBe('done');
    expect(statusLabel(status1, false).label).toBe('Done');

    // Last queued exchange should still show 'queued'
    const status2 = exchangeStatus(exchanges[2], '', true, true);
    expect(status2).toBe('queued');
    expect(statusLabel(status2, false).label).toBe('Queued');
  });

  it('clearing pending messages only removes the one whose real event arrived', () => {
    const { map, id } = makeThread();

    // Simulate thread with two pending messages
    const thread = map.get(id)!;
    thread.pendingUserMessages = [
      { text: 'Message A', eventId: 'msg-a', created: '2026-01-01T00:00:00Z' },
      { text: 'Message B', eventId: 'msg-b', created: '2026-01-01T00:00:00Z' },
    ];

    // Real MessageReceived arrives for 'Message A' — should only remove that one
    handleEvent(map, id, 1, { type: 'MessageReceived', text: 'Message A' }, '2026-01-01T00:00:00Z', 'msg-a');

    expect(thread.pendingUserMessages).toHaveLength(1);
    expect(thread.pendingUserMessages[0].eventId).toBe('msg-b');
  });

  it('non-last superseded exchange returns done (displayed as "Continued below")', () => {
    const { map, id } = makeThread();

    // First exchange is active
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'First', created: '2026-01-01T00:00:00Z' },
      { type: 'ToolCalled', name: 'search', args: {}, created: '2026-01-01T00:00:01Z' },
    ]);

    const exchanges = getExchanges(map, id);
    // Two queued exchanges — exchange[1] is superseded by exchange[2]
    exchanges.push({
      userEvent: { type: 'MessageReceived', text: 'Second' },
      userSeq: -1,
      steps: [],
    });
    exchanges.push({
      userEvent: { type: 'MessageReceived', text: 'Third' },
      userSeq: -2,
      steps: [],
    });

    // Exchange[1] is not last (superseded) → 'done' (ChatExchange shows "Continued below ↳")
    const status1 = exchangeStatus(exchanges[1], '', false, true);
    expect(status1).toBe('done');

    // Exchange[2] is last → 'queued'
    const status2 = exchangeStatus(exchanges[2], '', true, true);
    expect(status2).toBe('queued');
  });

  it('first completed exchange transitions queued messages to pending/streaming', () => {
    const { map, id } = makeThread();

    // First exchange completes
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'First', created: '2026-01-01T00:00:00Z' },
      { type: 'ResponseGenerated', text: 'Done!', created: '2026-01-01T00:00:05Z' },
    ]);

    // Second exchange starts (was queued, now has real events)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Second', created: '2026-01-01T00:00:06Z' },
      { type: 'TextStreamed', text: 'Working on second...', created: '2026-01-01T00:00:07Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    // Both are real exchanges (positive seqs). Standard isLast computation.
    // First: done (completed, not last)
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    // Second: streaming (last, prior is done → not active, has live streamingBuffer)
    const priorActive = false; // prior is 'done'
    expect(exchangeStatus(exchanges[1], 'Working on second...', true, priorActive)).toBe('streaming');
  });
});

// ---------------------------------------------------------------------------
// Bug: Pending CC follow-up exchanges show "CHAT" instead of "CLAUDE CODE"
// ---------------------------------------------------------------------------
describe('Bug: Pending CC exchanges must inherit thread channel', () => {
  /** Helper that mirrors store.ts activeExchanges logic for pending messages */
  function appendPendingExchanges(
    exchanges: Exchange[],
    pendingUserMessages: Array<{ text: string; eventId: string; created: string }>,
    threadSource: import('../thread-events').ThreadMeta['channel'],
  ): Exchange[] {
    for (let i = 0; i < pendingUserMessages.length; i++) {
      exchanges.push({
        userEvent: {
          type: 'MessageReceived',
          text: pendingUserMessages[i].text,
          channel: threadSource === 'error_unknown_channel' ? undefined : threadSource,
        },
        userSeq: -(i + 1),
        steps: [],
      });
    }
    return exchanges;
  }

  it('pending message in CC thread should have channel "claude_code"', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'CodingAgentIdled' },
    ]);

    const exchanges = getExchanges(map, id);
    const pending = [{ text: 'now fix tests', eventId: 'e1', created: '2026-01-01T00:00:00Z' }];
    appendPendingExchanges(exchanges, pending, map.get(id)!.meta.channel);

    expect(exchanges).toHaveLength(2);
    // The pending exchange must have channel "claude_code", not default to "chat"
    expect(exchangeUserChannel(exchanges[1])).toBe('claude_code');
  });

  it('pending message in regular chat thread should have channel "chat"', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'hello' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    const pending = [{ text: 'follow up', eventId: 'e2', created: '2026-01-01T00:00:00Z' }];
    appendPendingExchanges(exchanges, pending, map.get(id)!.meta.channel);

    expect(exchanges).toHaveLength(2);
    expect(exchangeUserChannel(exchanges[1])).toBe('chat');
  });

  it('CC follow-up pending message is removed when SSE event arrives with matching event_id', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    // Initial CC exchange
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentIdled' },
    ]);

    // Simulate pending follow-up
    const eventId = 'client-uuid-123';
    map.get(id)!.pendingUserMessages.push({ text: 'now fix tests', eventId, created: '2026-01-01T00:00:00Z' });
    expect(map.get(id)!.pendingUserMessages).toHaveLength(1);

    // Simulate SSE event arriving with matching event_id
    handleEvent(map, id, nextSeq(), {
      type: 'MessageReceived', text: 'now fix tests', channel: 'claude_code',
    }, TS, eventId);

    // Pending message must be removed
    expect(map.get(id)!.pendingUserMessages).toHaveLength(0);
  });

  it('CC follow-up pending message is NOT removed when SSE event has different event_id', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentIdled' },
    ]);

    // Simulate pending follow-up with client UUID
    map.get(id)!.pendingUserMessages.push({ text: 'now fix tests', eventId: 'client-uuid-123', created: '2026-01-01T00:00:00Z' });

    // SSE event arrives with DIFFERENT UUID (the bug: CC loop generates random UUID)
    handleEvent(map, id, nextSeq(), {
      type: 'MessageReceived', text: 'now fix tests', channel: 'claude_code',
    }, TS, 'random-server-uuid-456');

    // BUG: pending message stays because event_id doesn't match
    // After fix, this should be 0 — the CC loop must forward the client's event_id
    expect(map.get(id)!.pendingUserMessages).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// Bug: SSE-born scheduled trigger thread appears under Archive instead of Active
// ---------------------------------------------------------------------------
describe('Bug: SSE-born scheduled trigger thread categorization', () => {
  it('SSE-born skeleton thread derives running status after events load', () => {
    // SSE-born skeletons start with eventsLoaded: false. The drawer guards
    // status with eventsLoaded, so until loadThreadEvents completes, status
    // is displayed from the API metadata. After loading, SSE events update it.
    const id = 'scheduled-task-thread';
    const skeleton: ThreadState = {
      meta: {
        id,
        title: '...',
        channel: 'chat',
        initiator: 'user',
        saved: false,
        createdAt: '',
        updatedAt: '',
        status: 'idle',
        messageCount: 0,
        section: 'archived',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0, attentionDescendantCount: 0,
        codingAgentProposed: false,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        codingAgentHasDiff: false,
        lastRevivedAt: '',
        state: 'active',
        latestTodoList: null,
      },
      events: new Map(),
      streamingBuffer: '',
      eventsLoaded: false,  // SSE-born skeletons start unloaded
      eventsLoadFailed: false,
      lastDbSeq: 0,
      pendingUserMessages: [],
    };
    const map = new Map([[id, skeleton]]);

    // SSE delivers events for this thread — handleEvent updates meta.status
    handleEventWithAgg(map, id, nextSeq(), {
      type: 'TriggerStarted', trigger_id: 'task-1', trigger_name: 'Varmepumpe', prompt: 'Run control loop',
    } as any, new Date().toISOString());

    handleEventWithAgg(map, id, nextSeq(), {
      type: 'ToolCalled', name: 'execute_intent', args: { intent_id: 'heatpump' },
    } as any, new Date().toISOString());

    const thread = map.get(id)!;

    // After TriggerStarted, meta.status is updated to 'running'
    expect(thread.meta.status).toBe('running');

    // effectiveThreadStatus reads from meta.status
    thread.eventsLoaded = true;
    expect(effectiveThreadStatus(thread)).toBe('running');
  });

  it('scheduled trigger with TriggerStarted stays running until completion event', () => {
    const { map, id } = makeThread();
    const twoMinutesAgo = new Date(Date.now() - 120_000).toISOString();

    insertEvents(map, id, [
      { type: 'TriggerStarted', trigger_id: 't1', trigger_name: 'Test', prompt: 'run', created: twoMinutesAgo } as any,
      { type: 'ToolCalled', name: 'run_python', args: {}, created: twoMinutesAgo },
    ]);

    // TriggerStarted set 'running', no completion event → still running
    expect(map.get(id)!.meta.status).toBe('running');
  });
});

// ---------------------------------------------------------------------------
// Bug: pending message with no real events should still produce exchanges
// ---------------------------------------------------------------------------
describe('Bug: first message missing when only pending messages exist', () => {
  it('pending message should produce a synthetic exchange even with no real events', () => {
    const { map, id } = makeThread();
    const thread = map.get(id)!;

    // User sends first message — no SSE events yet, only pending
    thread.pendingUserMessages = [{ text: 'legg inn denne i kalenderen min', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }];

    // events.size is 0 — no real events from backend yet
    expect(thread.events.size).toBe(0);

    // ThreadView was checking `events.size > 0` to decide whether to show exchanges.
    // That's wrong — pending messages should also be considered "has data".
    const hasData = thread.events.size > 0 || thread.pendingUserMessages.length > 0;
    expect(hasData).toBe(true);

    // activeExchanges logic: groupIntoExchanges + append pending as synthetic
    const exchanges = groupIntoExchanges(thread.events);
    for (let i = 0; i < thread.pendingUserMessages.length; i++) {
      exchanges.push({
        userEvent: { type: 'MessageReceived', text: thread.pendingUserMessages[i].text, channel: thread.meta.channel === 'error_unknown_channel' ? undefined : thread.meta.channel },
        userSeq: -(i + 1),
        steps: [],
      });
    }

    // Must show 1 exchange — the pending message
    expect(exchanges).toHaveLength(1);
    expect(exchangeUserMessage(exchanges[0])).toBe('legg inn denne i kalenderen min');

    // Status should be 'pending' (no response yet)
    const status = exchangeStatus(exchanges[0], '', true);
    expect(status).toBe('pending');
  });

  it('thread status should be running when pending messages exist (effectiveThreadStatus)', () => {
    const { map, id } = makeThread();
    const thread = map.get(id)!;
    thread.pendingUserMessages = [{ text: 'hello', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }];

    // effectiveThreadStatus checks pendingUserMessages and returns 'running'
    const status = effectiveThreadStatus(thread);
    expect(status).toBe('running');
  });
});

// ---------------------------------------------------------------------------
// Bug: red status dot lingers on a failed thread while dismiss is in flight
// ---------------------------------------------------------------------------
