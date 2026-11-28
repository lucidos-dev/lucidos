import { describe, it, expect, beforeEach } from 'vitest';
import { getExchanges, getExchangesWithPending, getLabel, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { applySummaryVersion, changeReadyToReview, exchangeStatus, exchangeTimestamp, exchangeUserMessage, getCodingAgentWaitingInfo, groupIntoExchanges } from '../thread-events';
import { displaySection } from '../../generated/thread-lifecycle';
import { effectiveThreadStatus } from '../store';
import { _holdArchiveForTesting, _resetRowIntentsForTesting } from '../rowIntents';

beforeEach(resetSeqCounter);

describe('Bug: dismissed thread keeps red status dot until SSE round-trip lands', () => {
  it('effectiveThreadStatus returns idle once dismiss is requested, even on a failed thread', async () => {
    const { map, id } = makeThread('failed-thread');
    const thread = map.get(id)!;
    applySummaryVersion(thread.meta, thread.meta.summaryVersion, 'failed');

    expect(effectiveThreadStatus(thread)).toBe('failed');

    _holdArchiveForTesting([id]);
    try {
      expect(effectiveThreadStatus(thread)).toBe('idle');
    } finally {
      _resetRowIntentsForTesting();
    }
  });
});

describe('Bug: applying a change keeps thread in Review until CC actually runs', () => {
  it('effectiveThreadStatus does not flip to running just because Apply was clicked', async () => {
    const { applyNowRequests } = await import('../store');
    const { map, id } = makeThread('cc-with-changes');
    const thread = map.get(id)!;
    thread.meta.channel = 'claude_code';
    applySummaryVersion(thread.meta, thread.meta.summaryVersion, 'waiting');
    thread.meta.section = 'inbox';
    thread.meta.codingAgentChangeState = { kind: 'proposed', requires_restart: false };

    expect(effectiveThreadStatus(thread)).toBe('waiting');

    applyNowRequests.value = new Map([[id, null]]);
    try {
      expect(effectiveThreadStatus(thread)).toBe('waiting');

      const section = displaySection(
        thread.meta.section, effectiveThreadStatus(thread),
        thread.meta.saved, thread.meta.activeChildrenCount > 0,
        changeReadyToReview(thread.meta),
        thread.meta.attentionDescendantCount > 0,
        false,
      );
      expect(section).toBe('current');
    } finally {
      applyNowRequests.value = new Map();
    }
  });
});

describe('Bug: CC follow-up creates proper exchange boundary with pending messages', () => {

  it('CC events arriving after follow-up should go into the new exchange, not the old one', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';
    const t0 = '2026-03-17T21:01:30.000Z';
    const t1 = '2026-03-17T21:01:35.000Z';
    const t2 = '2026-03-17T21:01:36.000Z';
    const t3 = '2026-03-17T21:01:37.000Z';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: t0 },
      { type: 'SessionStarted', session_id: 's1', created: t0 },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t1 },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t1 },
    ]);

    map.get(id)!.pendingUserMessages.push({
      text: 'sorry wrong thread',
      eventId: 'follow-up-1',
      created: t2,
    } as any);

    insertEvents(map, id, [
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t3 },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t3 },
    ]);

    const exchanges = getExchangesWithPending(map, id);

    expect(exchanges).toHaveLength(2);

    expect(exchangeUserMessage(exchanges[0])).toBe('fix the bug');
    expect(exchanges[0].steps.length).toBe(3);

    expect(exchangeUserMessage(exchanges[1])).toBe('sorry wrong thread');
    expect(exchanges[1].steps.length).toBe(2);
  });

  it('old exchange should show interrupted status when follow-up pending message exists', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';
    const t0 = '2026-03-17T21:01:30.000Z';
    const t1 = '2026-03-17T21:01:35.000Z';
    const t2 = '2026-03-17T21:01:36.000Z';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: t0 },
      { type: 'SessionStarted', session_id: 's1', created: t0 },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t1 },
    ]);

    map.get(id)!.pendingUserMessages.push({
      text: 'sorry wrong thread',
      eventId: 'follow-up-1',
      created: t2,
    } as any);

    const exchanges = getExchangesWithPending(map, id);
    expect(exchanges).toHaveLength(2);

    const status0 = exchangeStatus(exchanges[0], '', false, false, true);
    expect(status0).toBe('interrupted');

    const status1 = exchangeStatus(exchanges[1], '', true, true, true);
    expect(status1).toBe('pending');
  });

  it('old append-after approach incorrectly puts CC events in old exchange (demonstrates bug)', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';
    const t0 = '2026-03-17T21:01:30.000Z';
    const t1 = '2026-03-17T21:01:35.000Z';
    const t3 = '2026-03-17T21:01:37.000Z';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: t0 },
      { type: 'SessionStarted', session_id: 's1', created: t0 },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t1 },
    ]);

    insertEvents(map, id, [
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t3 },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t3 },
    ]);

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].steps.length).toBe(4);

    map.get(id)!.pendingUserMessages.push({
      text: 'sorry wrong thread',
      eventId: 'follow-up-1',
      created: '2026-03-17T21:01:36.000Z',
    } as any);
    const fixed = getExchangesWithPending(map, id);
    expect(fixed).toHaveLength(2);
    expect(fixed[0].steps.length).toBe(2);
    expect(fixed[1].steps.length).toBe(2);
  });

  it('pending follow-up timestamp should be stable, not change on re-render', () => {
    const { map, id } = makeThread();
    const created = '2026-03-17T21:01:39.000Z';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'hello', created: '2026-03-17T21:01:30.000Z' } as any,
      { type: 'ResponseGenerated', created: '2026-03-17T21:01:35.000Z' } as any,
    ]);

    map.get(id)!.pendingUserMessages.push({
      text: 'follow up',
      eventId: 'e1',
      created,
    } as any);

    const exchanges = getExchangesWithPending(map, id);
    expect(exchanges).toHaveLength(2);

    const ts = exchangeTimestamp(exchanges[1]);
    expect(ts).toBe(created);
  });
});

describe('Flow: CC revival from waiting', () => {
  it('CC resumes work in same exchange after CodingAgentIdled → status becomes coding-agent-working', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'CodingAgentTextStreamed', text: 'Fixed.' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled' },
      { type: 'CodingAgentToolCalled', name: 'Grep', args: {} },
      { type: 'CodingAgentToolResult', name: 'Grep', result: 'results' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('coding-agent-working');
    expect(getLabel(exchanges[0])).toBe('Working');
  });

  it('CC goes idle, resumes, then goes idle again → status is done', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'CodingAgentIdled' },
      { type: 'CodingAgentToolCalled', name: 'Grep', args: {} },
      { type: 'CodingAgentToolResult', name: 'Grep', result: 'results' },
      { type: 'CodingAgentIdled' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    expect(getLabel(exchanges[0])).toBe('Done');
  });

  it('CC follow-up creates new exchange — old exchange becomes done, new is coding-agent-working', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'CodingAgentIdled' },
    ]);

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'now fix tests', channel: 'claude_code' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: {} },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'tests pass' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false, false, true)).toBe('done');
    expect(getLabel(exchanges[0], '', false, false, true)).toBe('Done');
    expect(exchangeStatus(exchanges[1], '', true, false, true)).toBe('coding-agent-working');
    expect(getLabel(exchanges[1], '', true, false, true)).toBe('Working');
  });

  it('CodingAgentPromptSent after idle resets exchange status to coding-agent-working', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'CodingAgentIdled' },
      { type: 'CodingAgentPromptSent', text: '/harden' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('coding-agent-working');
    expect(getLabel(exchanges[0])).toBe('Working');

    const thread = map.get(id)!;
    expect(thread.meta.status).toBe('running');
  });

  it('CodingAgentPromptSent after idle + more work → coding-agent-working', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentIdled' },
      { type: 'CodingAgentPromptSent', text: '/harden' },
      { type: 'CodingAgentToolCalled', name: 'Grep', args: {} },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('coding-agent-working');
  });

  it('CodingAgentPromptSent after idle then idle again → done', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentIdled' },
      { type: 'CodingAgentPromptSent', text: '/harden' },
      { type: 'CodingAgentToolCalled', name: 'Grep', args: {} },
      { type: 'CodingAgentIdled' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
  });

  it('thread status is running when CC resumes after idle (CodingAgentPromptSent)', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentIdled' },
      { type: 'CodingAgentPromptSent', text: 'continue' },
    ]);

    const thread = map.get(id)!;
    expect(thread.meta.status).toBe('running');
  });


  it('thread status is running during hardening handoff (hardening session active)', () => {
    const { map, id } = makeThread();
    const now = new Date();
    const t = (offsetMs: number) => new Date(now.getTime() + offsetMs).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'add feature X', channel: 'claude_code', created: t(-15000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-14000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-13000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-12000) },
      { type: 'ResponseGenerated', created: t(-11000) },
      { type: 'CodingAgentIdled', created: t(-10000) },
      { type: 'SessionEnded', created: t(-9000) },
      { type: 'SessionStarted', session_id: 's2', created: t(-4000) },
      { type: 'CodingAgentPromptSent', text: 'Run /harden now.', created: t(-3500) },
      { type: 'CodingAgentToolCalled', name: 'Grep', args: { pattern: 'test' }, created: t(-3000) },
      { type: 'CodingAgentToolResult', name: 'Grep', result: 'found', created: t(-2000) },
      { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'src/main.rs' }, created: t(-1000) },
    ]);

    const thread = map.get(id)!;
    expect(thread.meta.status).toBe('running');
  });

  it('thread status is idle during hardening handoff gap (no hardening events yet)', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'add feature X', channel: 'claude_code', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-01-01T00:00:03Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:04Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:05Z' },
      { type: 'SessionEnded', created: '2026-01-01T00:00:06Z' },
    ]);

    const thread = map.get(id)!;
    expect(thread.meta.status).toBe('idle');
  });

  it('thread status is idle after hardening completes with proposed changes', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'add feature X', channel: 'claude_code', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:01Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:03Z' },
      { type: 'SessionEnded', created: '2026-01-01T00:00:04Z' },
      { type: 'SessionStarted', session_id: 's2', created: '2026-01-01T00:00:05Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:00:06Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-01-01T00:00:07Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:08Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:09Z' },
      { type: 'ChangeProposed', change_id: 'c1', created: '2026-01-01T00:00:10Z' },
      { type: 'SessionEnded', created: '2026-01-01T00:00:11Z' },
    ]);

    const thread = map.get(id)!;
    expect(thread.meta.status).toBe('idle');
  });
});

describe('Flow: MissingHardeningDetected', () => {
  it('ResponseCanceled sets idle, MissingHardeningDetected does not change status', () => {
    const { map, id } = makeThread();
    const now = new Date();
    const t = (offsetMs: number) => new Date(now.getTime() + offsetMs).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix title bug', created: t(-10000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-9000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-8000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-7000) },
      { type: 'CodingAgentTextStreamed', text: 'Nothing more needed here.', created: t(-6000) },
      { type: 'ResponseCanceled', created: t(-5000) },
      { type: 'MissingHardeningDetected', created: t(-4000) } as any,
    ]);

    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('thread is running during hardening session after MissingHardeningDetected', () => {
    const { map, id } = makeThread();
    const now = new Date();
    const t = (offsetMs: number) => new Date(now.getTime() + offsetMs).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix title bug', created: t(-10000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-9000) },
      { type: 'CodingAgentTextStreamed', text: 'Fixed.', created: t(-8000) },
      { type: 'ResponseCanceled', created: t(-7000) },
      { type: 'MissingHardeningDetected', created: t(-6000) } as any,
      { type: 'CodingAgentPromptSent', text: 'Run /harden now.', created: t(-3000) },
      { type: 'SessionStarted', session_id: 's2', created: t(-2000) },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: {}, created: t(-1000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('running');
  });

  it('MissingHardeningDetected opens its own initiator-panel exchange', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix bug' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'ResponseCanceled' },
      { type: 'MissingHardeningDetected' } as any,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[1].userEvent.type).toBe('ResponseCanceled');
    expect(exchanges[2].userEvent.type).toBe('MissingHardeningDetected');
    expect(exchangeUserMessage(exchanges[2])).toBe('Lucidos Engine — Hardening');
  });

  it('MissingHardeningDetected clears CC waiting state (no stale Apply/Discard buttons)', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix bug' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'CodingAgentIdled', has_changes: true },
      { type: 'MissingHardeningDetected' } as any,
    ]);

    const thread = map.get(id)!;
    expect(getCodingAgentWaitingInfo(thread.meta)).toBeNull();
  });
});

describe('SSE event-based status transitions via handleEvent()', () => {
  it('CC thread with SessionEnded + CodingAgentIdled → idle (SessionEnded transitions to idle)', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-01-01T00:00:03Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:04Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:05Z' },
      { type: 'SessionEnded', created: '2026-01-01T00:00:06Z' },
    ]);

    const thread = map.get(id)!;

    expect(thread.meta.status).toBe('idle');
  });

  it('CC thread with SessionEnded + pending changes → idle', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:01Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentIdled', has_changes: true, created: '2026-01-01T00:00:03Z' },
      { type: 'ChangeProposed', change_id: 'c1', created: '2026-01-01T00:00:04Z' },
      { type: 'SessionEnded', created: '2026-01-01T00:00:05Z' },
    ]);

    const thread = map.get(id)!;

    expect(thread.meta.status).toBe('idle');
    expect(thread.meta.codingAgentChangeState.kind).toBe('proposed');
  });

  it('CodingAgentIdled sets status=idle', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);

    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('MessageReceived + ToolCalled → status=running (MessageReceived sets running)', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'hello' },
      { type: 'ToolCalled', name: 'run_python', args: {} },
    ]);

    expect(map.get(id)!.meta.status).toBe('running');
  });

  it('ResponseGenerated on chat thread → status=idle', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'hello', created: '2026-01-01T00:00:00Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:01Z' },
    ]);

    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('ChangeApplied clears CC flags and sets status=idle', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it', created: t(-60000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-59000) },
      { type: 'ResponseGenerated', created: t(-55000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-54000) },
      { type: 'ChangeProposed', change_id: 'c1', created: t(-50000) },
      { type: 'ChangeApplied', change_id: 'c1', created: t(-10000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('idle');
    expect(map.get(id)!.meta.codingAgentChangeState).toEqual({ kind: 'none' });
  });

  it('ChangeDiscarded clears CC flags and sets status=idle', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it', created: t(-60000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-59000) },
      { type: 'ResponseGenerated', created: t(-55000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-54000) },
      { type: 'ChangeProposed', change_id: 'c1', created: t(-50000) },
      { type: 'ChangeDiscarded', change_id: 'c1', created: t(-10000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('idle');
    expect(map.get(id)!.meta.codingAgentChangeState.kind).not.toBe('proposed');
  });
});

describe('ResponseGenerated sets status=idle (when no pending changes)', () => {
  it('chat thread with ResponseGenerated → idle', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'start 5 CC tasks', channel: 'chat', created: '2026-03-20T21:18:56Z' },
      { type: 'TextStreamed', text: 'Starting tasks...', created: '2026-03-20T21:19:21Z' },
      { type: 'ToolCalled', name: 'start_claude_code', args: {}, created: '2026-03-20T21:19:21Z' },
      { type: 'ToolResult', name: 'start_claude_code', result: 'ok', created: '2026-03-20T21:19:21Z' },
      { type: 'TextStreamed', text: 'Tasks started', created: '2026-03-20T21:19:59Z' },
      { type: 'ResponseGenerated', created: '2026-03-20T21:19:59Z' },
    ]);

    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('chat thread with multiple exchanges → idle after last ResponseGenerated', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'hello', created: '2026-03-20T21:18:00Z' },
      { type: 'ResponseGenerated', created: '2026-03-20T21:18:10Z' },
      { type: 'MessageReceived', text: 'follow up', created: '2026-03-20T21:22:00Z' },
      { type: 'ResponseGenerated', created: '2026-03-20T21:22:10Z' },
    ]);

    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('scheduled trigger with ResponseGenerated → idle (TriggerCompleted is better, but fallback works)', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'TriggerStarted', trigger_id: 'task-1', created: '2026-03-20T08:00:00Z' },
      { type: 'ResponseGenerated', created: '2026-03-20T08:00:30Z' },
    ]);

    expect(map.get(id)!.meta.status).toBe('idle');
  });
});

describe('SessionStarted does not alter thread status', () => {
  it('ChangeApplied + CodingAgentIdled(no changes) + SessionStarted → idle preserved', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it', channel: 'claude_code', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-110000) },
      { type: 'ResponseGenerated', created: t(-100000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-99000) },
      { type: 'ChangeProposed', change_id: 'c1', created: t(-98000) },
      { type: 'ChangeApplied', change_id: 'c1', created: t(-90000) },
      { type: 'CodingAgentIdled', created: t(-89000) },
      { type: 'SessionStarted', session_id: 's2', created: t(-80000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('ChangeApplied + SessionStarted → idle (SessionStarted does not change status)', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it', channel: 'claude_code', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-110000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-99000) },
      { type: 'ChangeProposed', change_id: 'c1', created: t(-98000) },
      { type: 'ChangeApplied', change_id: 'c1', created: t(-90000) },
      { type: 'SessionStarted', session_id: 's2', created: t(-80000) },
    ]);

    const status = map.get(id)!.meta.status;
    expect(status).toBe('idle');

    expect(displaySection('archived', status, false, false, false, false, false)).toBe('archive');
  });

  it('ChangeDiscarded + SessionStarted → idle (SessionStarted does not change status)', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it', channel: 'claude_code', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-99000) },
      { type: 'ChangeProposed', change_id: 'c1', created: t(-98000) },
      { type: 'ChangeDiscarded', change_id: 'c1', created: t(-90000) },
      { type: 'SessionStarted', session_id: 's2', created: t(-80000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('pending changes + SessionStarted → idle (SessionStarted does not change status)', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it', channel: 'claude_code', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-200000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-199000) },
      { type: 'ResponseGenerated', created: t(-180000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-179000) },
      { type: 'ChangeProposed', change_id: 'c1', created: t(-178000) },
      { type: 'SessionStarted', session_id: 's2', created: t(-120000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('idle');
  });

});
