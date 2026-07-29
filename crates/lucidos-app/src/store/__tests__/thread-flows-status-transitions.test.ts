import { describe, it, expect, beforeEach } from 'vitest';
import { getExchanges, getExchangesWithPending, getLabel, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { exchangeStatus, exchangeTimestamp, exchangeUserMessage, getCodingAgentWaitingInfo, groupIntoExchanges } from '../thread-events';
import { displaySection } from '../../generated/thread-lifecycle';
import { effectiveThreadStatus } from '../store';

beforeEach(resetSeqCounter);

describe('Bug: dismissed thread keeps red status dot until SSE round-trip lands', () => {
  it('effectiveThreadStatus returns idle once dismiss is requested, even on a failed thread', async () => {
    const { archivingThreadIds } = await import('../store');
    const { map, id } = makeThread('failed-thread');
    const thread = map.get(id)!;
    thread.meta.status = 'failed';

    // Without dismiss in flight: red dot status surfaces.
    expect(effectiveThreadStatus(thread)).toBe('failed');

    // User clicks dismiss → optimistic state added before SSE arrives.
    archivingThreadIds.value = new Set([id]);
    try {
      expect(effectiveThreadStatus(thread)).toBe('idle');
    } finally {
      archivingThreadIds.value = new Set();
    }
  });
});

// ---------------------------------------------------------------------------
// Bug: applying a change should not move the thread to the Active section
// ---------------------------------------------------------------------------
describe('Bug: applying a change keeps thread in Review until CC actually runs', () => {
  it('effectiveThreadStatus does not flip to running just because Apply was clicked', async () => {
    const { applyingNowThreadIds } = await import('../store');
    const { map, id } = makeThread('cc-with-changes');
    const thread = map.get(id)!;
    thread.meta.channel = 'claude_code';
    thread.meta.status = 'waiting';
    thread.meta.section = 'inbox';
    thread.meta.codingAgentProposed = true;

    expect(effectiveThreadStatus(thread)).toBe('waiting');

    applyingNowThreadIds.value = new Map([[id, 'requesting']]);
    try {
      // Status must stay 'waiting' — only CC activity events (or harden/conflict
      // boundary events that precede them) should flip the thread to running.
      expect(effectiveThreadStatus(thread)).toBe('waiting');

      // displaySection then routes to Current.
      const section = displaySection(
        thread.meta.section, effectiveThreadStatus(thread),
        thread.meta.saved, thread.meta.activeChildrenCount > 0,
        thread.meta.codingAgentProposed,
        thread.meta.attentionDescendantCount > 0,
      );
      expect(section).toBe('current');
    } finally {
      applyingNowThreadIds.value = new Map();
    }
  });
});

// ---------------------------------------------------------------------------
// Bug: CC follow-up response appends to previous exchange instead of new one
// ---------------------------------------------------------------------------
describe('Bug: CC follow-up creates proper exchange boundary with pending messages', () => {

  it('CC events arriving after follow-up should go into the new exchange, not the old one', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';
    const t0 = '2026-03-17T21:01:30.000Z';
    const t1 = '2026-03-17T21:01:35.000Z';
    const t2 = '2026-03-17T21:01:36.000Z';
    const t3 = '2026-03-17T21:01:37.000Z';

    // Initial CC exchange — CC is working
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: t0 },
      { type: 'SessionStarted', session_id: 's1', created: t0 },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t1 },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t1 },
    ]);

    // User sends follow-up while CC is still working — pending message created
    map.get(id)!.pendingUserMessages.push({
      text: 'sorry wrong thread',
      eventId: 'follow-up-1',
      created: t2,  // client timestamp when message was sent
    } as any);

    // CC events continue arriving AFTER the follow-up was sent
    // (these have server timestamps after the pending message's timestamp)
    insertEvents(map, id, [
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t3 },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t3 },
    ]);

    const exchanges = getExchangesWithPending(map, id);

    // Must have 2 exchanges: old task + follow-up
    expect(exchanges).toHaveLength(2);

    // Exchange 1: old task (events before follow-up)
    expect(exchangeUserMessage(exchanges[0])).toBe('fix the bug');
    // Should have 3 steps (SessionStarted + ToolCalled + ToolResult before follow-up)
    expect(exchanges[0].steps.length).toBe(3);

    // Exchange 2: follow-up (events after follow-up)
    expect(exchangeUserMessage(exchanges[1])).toBe('sorry wrong thread');
    // CC events after the follow-up should be in THIS exchange, not Exchange 1
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

    // User sends follow-up
    map.get(id)!.pendingUserMessages.push({
      text: 'sorry wrong thread',
      eventId: 'follow-up-1',
      created: t2,
    } as any);

    const exchanges = getExchangesWithPending(map, id);
    expect(exchanges).toHaveLength(2);

    // Old exchange should NOT be 'coding-agent-working' — it should be 'interrupted'
    // because there's a newer exchange after it
    const status0 = exchangeStatus(exchanges[0], '', false, false, true);
    expect(status0).toBe('interrupted');

    // Follow-up should show as pending (CC doesn't queue like chat)
    const status1 = exchangeStatus(exchanges[1], '', true, true, true);
    expect(status1).toBe('pending');
  });

  it('old append-after approach incorrectly puts CC events in old exchange (demonstrates bug)', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';
    const t0 = '2026-03-17T21:01:30.000Z';
    const t1 = '2026-03-17T21:01:35.000Z';
    const t3 = '2026-03-17T21:01:37.000Z';

    // Initial CC exchange + CC continues working
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: t0 },
      { type: 'SessionStarted', session_id: 's1', created: t0 },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t1 },
    ]);

    // User sends follow-up (pending message, not yet in events)
    // CC events arrive AFTER follow-up was sent
    insertEvents(map, id, [
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t3 },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t3 },
    ]);

    // OLD approach: groupIntoExchanges doesn't know about pending messages
    const exchanges = groupIntoExchanges(map.get(id)!.events);
    // BUG: only 1 exchange — all CC events (including post-follow-up) are in the old exchange
    expect(exchanges).toHaveLength(1);
    // All 4 steps are in the old exchange — the post-follow-up events leak into it
    expect(exchanges[0].steps.length).toBe(4);

    // With the fix (getExchangesWithPending), the post-follow-up events would be in Exchange 2
    map.get(id)!.pendingUserMessages.push({
      text: 'sorry wrong thread',
      eventId: 'follow-up-1',
      created: '2026-03-17T21:01:36.000Z',
    } as any);
    const fixed = getExchangesWithPending(map, id);
    expect(fixed).toHaveLength(2);
    expect(fixed[0].steps.length).toBe(2);  // SessionStarted + ToolCalled (before follow-up)
    expect(fixed[1].steps.length).toBe(2);  // ToolCalled + ToolResult (after follow-up)
  });

  it('pending follow-up timestamp should be stable, not change on re-render', () => {
    const { map, id } = makeThread();
    const created = '2026-03-17T21:01:39.000Z';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'hello', created: '2026-03-17T21:01:30.000Z' } as any,
      { type: 'ResponseGenerated', created: '2026-03-17T21:01:35.000Z' } as any,
    ]);

    // Pending message with explicit created timestamp
    map.get(id)!.pendingUserMessages.push({
      text: 'follow up',
      eventId: 'e1',
      created,
    } as any);

    const exchanges = getExchangesWithPending(map, id);
    expect(exchanges).toHaveLength(2);

    // Timestamp should use the stored created value, not new Date()
    const ts = exchangeTimestamp(exchanges[1]);
    expect(ts).toBe(created);
  });
});

// ---------------------------------------------------------------------------
// Flow: CC revival — CC resumes after idle
// ---------------------------------------------------------------------------
describe('Flow: CC revival from waiting', () => {
  it('CC resumes work in same exchange after CodingAgentIdled → status becomes coding-agent-working', () => {
    const { map, id } = makeThread();

    // Claude Code session: works, goes idle, then resumes (more tool calls arrive)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'CodingAgentTextStreamed', text: 'Fixed.' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled' },
      // CC resumes — more work events arrive after idle
      { type: 'CodingAgentToolCalled', name: 'Grep', args: {} },
      { type: 'CodingAgentToolResult', name: 'Grep', result: 'results' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    // Status should be coding-agent-working (not done) because CC resumed
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
      // CC resumes
      { type: 'CodingAgentToolCalled', name: 'Grep', args: {} },
      { type: 'CodingAgentToolResult', name: 'Grep', result: 'results' },
      // CC goes idle again
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

    // First exchange: CC works and goes idle
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'CodingAgentIdled' },
    ]);

    // User sends follow-up → new exchange
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'now fix tests', channel: 'claude_code' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: {} },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'tests pass' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // Old exchange: was idle, now not last → done
    expect(exchangeStatus(exchanges[0], '', false, false, true)).toBe('done');
    expect(getLabel(exchanges[0], '', false, false, true)).toBe('Done');
    // New exchange: actively working
    expect(exchangeStatus(exchanges[1], '', true, false, true)).toBe('coding-agent-working');
    expect(getLabel(exchanges[1], '', true, false, true)).toBe('Working');
  });

  it('CodingAgentPromptSent after idle resets exchange status to coding-agent-working', () => {
    // Bug: CodingAgentPromptSent (automated prompt, e.g. hardening/conflict resolution)
    // was not handled in exchangeStatus, so isCCWaiting stayed true → 'done'.
    // Meanwhile the backend status correctly showed 'running' (active Claude Code session).
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'CodingAgentIdled' },
      // Engine sends automated prompt (e.g. hardening) — CC resumes
      { type: 'CodingAgentPromptSent', text: '/harden' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    // Exchange should be coding-agent-working (not done) — CC is processing the automated prompt
    expect(exchangeStatus(exchanges[0], '', true)).toBe('coding-agent-working');
    expect(getLabel(exchanges[0])).toBe('Working');

    // Thread status should also be running (CC activity after completion)
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
      // CC resumes — prompt sent is a status-changing event
      { type: 'CodingAgentPromptSent', text: 'continue' },
    ]);

    const thread = map.get(id)!;
    expect(thread.meta.status).toBe('running');
  });

  // ---------------------------------------------------------------------------
  // Hardening handoff: original session ends, hardening session starts
  // ---------------------------------------------------------------------------

  it('thread status is running during hardening handoff (hardening session active)', () => {
    // Scenario: original Claude Code session finishes → SessionEnded → review session starts
    // The hardening session is actively hardening (tool calls in progress).
    // Thread should be 'running', not 'idle'.
    // Use fresh timestamps — this happens in real-time.
    const { map, id } = makeThread();
    const now = new Date();
    const t = (offsetMs: number) => new Date(now.getTime() + offsetMs).toISOString();

    insertEvents(map, id, [
      // Original Claude Code session
      { type: 'MessageReceived', text: 'add feature X', channel: 'claude_code', created: t(-15000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-14000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-13000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-12000) },
      { type: 'ResponseGenerated', created: t(-11000) },
      { type: 'CodingAgentIdled', created: t(-10000) },
      // Original session ends, hands off to hardening
      { type: 'SessionEnded', created: t(-9000) },
      // Hardening Claude Code session starts and is actively working
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
    // Scenario: original Claude Code session ended, review session hasn't started yet.
    // This is the transient gap — thread correctly shows as idle because
    // SessionEnded is the last event with no subsequent start event.
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'add feature X', channel: 'claude_code', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-01-01T00:00:03Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:04Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:05Z' },
      { type: 'SessionEnded', created: '2026-01-01T00:00:06Z' },
      // No review session events yet — gap
    ]);

    const thread = map.get(id)!;
    // SessionEnded with no pending changes → idle (this is the transient gap)
    expect(thread.meta.status).toBe('idle');
  });

  it('thread status is waiting after hardening completes with proposed changes', () => {
    // Review Claude Code session completed, proposed a change, then ended.
    // Thread should show as 'waiting' (pending change), not 'idle'.
    const { map, id } = makeThread();

    insertEvents(map, id, [
      // Original Claude Code session
      { type: 'MessageReceived', text: 'add feature X', channel: 'claude_code', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:01Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:03Z' },
      { type: 'SessionEnded', created: '2026-01-01T00:00:04Z' },
      // Review session
      { type: 'SessionStarted', session_id: 's2', created: '2026-01-01T00:00:05Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:00:06Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-01-01T00:00:07Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:08Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:09Z' },
      { type: 'ChangeProposed', change_id: 'c1', created: '2026-01-01T00:00:10Z' },
      { type: 'SessionEnded', created: '2026-01-01T00:00:11Z' },
    ]);

    const thread = map.get(id)!;
    // ChangeProposed without ChangeApplied/Discarded → pending changes → waiting
    expect(thread.meta.status).toBe('waiting');
  });
});

// ---------------------------------------------------------------------------
// MissingHardeningDetected — hardening recovery flow
// ---------------------------------------------------------------------------
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
      // MissingHardeningDetected is not a status-changing event
      { type: 'MissingHardeningDetected', created: t(-4000) } as any,
    ]);

    // ResponseCanceled set idle (no codingAgentProposed). MissingHardeningDetected doesn't change status.
    // The hardening session (SessionStarted) will set it back to 'running' when it starts.
    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('thread is running during hardening session after MissingHardeningDetected', () => {
    const { map, id } = makeThread();
    const now = new Date();
    const t = (offsetMs: number) => new Date(now.getTime() + offsetMs).toISOString();

    insertEvents(map, id, [
      // Original Claude Code session (fresh timestamps — real-time flow)
      { type: 'MessageReceived', text: 'fix title bug', created: t(-10000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-9000) },
      { type: 'CodingAgentTextStreamed', text: 'Fixed.', created: t(-8000) },
      { type: 'ResponseCanceled', created: t(-7000) },
      // Hardening detection
      { type: 'MissingHardeningDetected', created: t(-6000) } as any,
      // Review session starts
      { type: 'CodingAgentPromptSent', text: 'Run /harden now.', created: t(-3000) },
      { type: 'SessionStarted', session_id: 's2', created: t(-2000) },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: {}, created: t(-1000) },
    ]);

    // Thread must be running while review is in progress (live process)
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
    // 1: original CC reply, 2: ResponseCanceled boundary, 3: MissingHardeningDetected
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
      // Engine detects missing hardening — should clear the idle/waiting state
      { type: 'MissingHardeningDetected' } as any,
    ]);

    const thread = map.get(id)!;
    // CC waiting info should be null — no Apply/Discard buttons
    expect(getCodingAgentWaitingInfo(thread.meta)).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// SSE event-based status transitions
// ---------------------------------------------------------------------------
describe('SSE event-based status transitions via handleEvent()', () => {
  it('CC thread with SessionEnded + CodingAgentIdled → idle (SessionEnded transitions to idle)', () => {
    // Backend emits SessionEnded when Claude Code session completes. This transitions to idle.
    const { map, id } = makeThread();

    // Completed session events (loaded from DB)
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

    // SessionEnded checks codingAgentProposed (false) → idle
    expect(thread.meta.status).toBe('idle');
  });

  it('CC thread with SessionEnded + pending changes → waiting', () => {
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

    // SessionEnded checks codingAgentProposed (true) → waiting
    expect(thread.meta.status).toBe('waiting');
    expect(thread.meta.codingAgentProposed).toBe(true);
  });

  it('CodingAgentIdled sets status=waiting', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);

    // CodingAgentIdled always sets waiting
    expect(map.get(id)!.meta.status).toBe('waiting');
  });

  it('MessageReceived + ToolCalled → status=running (MessageReceived sets running)', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'hello' },
      { type: 'ToolCalled', name: 'run_python', args: {} },
    ]);

    // MessageReceived sets status='running'
    expect(map.get(id)!.meta.status).toBe('running');
  });

  it('ResponseGenerated on chat thread → status=idle', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'hello', created: '2026-01-01T00:00:00Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:01Z' },
    ]);

    // ResponseGenerated checks codingAgentProposed (false) → idle
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

    // ChangeApplied sets status='idle' and clears all CC flags
    expect(map.get(id)!.meta.status).toBe('idle');
    expect(map.get(id)!.meta.codingAgentProposed).toBe(false);
    expect(map.get(id)!.meta.codingAgentRequiresRestart).toBe(false);
    expect(map.get(id)!.meta.codingAgentApplying).toBe(false);
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

    // ChangeDiscarded sets status='idle' and clears all CC flags
    expect(map.get(id)!.meta.status).toBe('idle');
    expect(map.get(id)!.meta.codingAgentProposed).toBe(false);
  });
});

// ResponseGenerated transitions to idle
// ---------------------------------------------------------------------------
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

    // ResponseGenerated checks codingAgentProposed (false) → idle
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

    // Last ResponseGenerated sets idle
    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('scheduled trigger with ResponseGenerated → idle (TriggerCompleted is better, but fallback works)', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'TriggerStarted', trigger_id: 'task-1', created: '2026-03-20T08:00:00Z' },
      { type: 'ResponseGenerated', created: '2026-03-20T08:00:30Z' },
    ]);

    // ResponseGenerated sets idle (TriggerCompleted is better, but this works)
    expect(map.get(id)!.meta.status).toBe('idle');
  });
});

// SessionStarted is metadata — it never alters thread status
// ---------------------------------------------------------------------------
describe('SessionStarted does not alter thread status', () => {
  it('ChangeApplied + CodingAgentIdled(no changes) + SessionStarted → idle preserved', () => {
    // SessionStarted is a metadata event. It must not change status.
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
      { type: 'CodingAgentIdled', created: t(-89000) },  // has_changes=false (omitted)
      { type: 'SessionStarted', session_id: 's2', created: t(-80000) },
    ]);

    // SessionStarted is metadata — doesn't change status. CodingAgentIdled without
    // has_changes after ChangeApplied correctly goes idle.
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
    // ChangeApplied set idle, SessionStarted doesn't change it
    expect(status).toBe('idle');

    // displaySection with idle status + default section → archive
    expect(displaySection('archived', status, false, false, false, false)).toBe('archive');
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

  it('pending changes + SessionStarted → waiting (SessionStarted does not change status)', () => {
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

    // ChangeProposed set waiting with codingAgentProposed. SessionStarted doesn't change it.
    expect(map.get(id)!.meta.status).toBe('waiting');
  });

});

// Bug: Claude Code session aborted by engine restart should show as needing attention
// The engine no longer emits CodingAgentIdled during shutdown, so the last event
// is ResponseAborted → thread should be in review (inbox), not idle/archive.
// ---------------------------------------------------------------------------
