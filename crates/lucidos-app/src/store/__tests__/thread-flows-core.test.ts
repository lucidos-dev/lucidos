import { describe, it, expect, beforeEach } from 'vitest';
import { getExchanges, getLabel, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { exchangeError, exchangeResponseEvents, exchangeResponseText, exchangeStatus, exchangeSteps, exchangeUserChannel, exchangeUserMessage, exchangeUserSource, groupIntoExchanges, handleEvent, type ThreadEvent } from '../thread-events';
import { getCollapsedVisibleEvents, getEventToggleState } from '../event-rendering';

beforeEach(resetSeqCounter);

describe('Flow: New chat message', () => {
  it('MessageReceived + ToolCalled shows as streaming with steps', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'What is 2+2?' },
      { type: 'ToolCalled', name: 'calculator', args: {} },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeUserMessage(exchanges[0])).toBe('What is 2+2?');
    expect(exchangeSteps(exchanges[0]).length).toBeGreaterThan(0);
    // Has steps but no response → streaming → label "Working"
    expect(exchangeStatus(exchanges[0], '', true)).toBe('streaming');
    expect(getLabel(exchanges[0])).toBe('Working');
  });

  it('complete flow: MessageReceived → tools → text → ResponseGenerated', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'What is 2+2?' },
      { type: 'ToolCalled', name: 'calculator', args: { expr: '2+2' } },
      { type: 'ToolResult', name: 'calculator', result: '4' },
      { type: 'TextStreamed', text: 'The answer is 4.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeUserMessage(exchanges[0])).toBe('What is 2+2?');
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    expect(getLabel(exchanges[0])).toBe('Done');
    expect(exchangeResponseText(exchanges[0])).toBe('The answer is 4.');
    expect(exchangeSteps(exchanges[0])).toHaveLength(1);
    expect(exchangeSteps(exchanges[0])[0].description).toBe('Calculator');
    expect(exchangeSteps(exchanges[0])[0].outcome).toBe('success');

    // Events should have step + text interleaved
    const events = exchangeResponseEvents(exchanges[0]);
    const stepEvents = events.filter(e => e.type === 'step');
    const textEvents = events.filter(e => e.type === 'text');
    expect(stepEvents).toHaveLength(1);
    expect(stepEvents[0].outcome).toBe('success');
    expect(textEvents).toHaveLength(1);
    expect((textEvents[0] as { md: string }).md).toBe('The answer is 4.');
  });

  it('ResponseFailed shows error status', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Do something' },
      { type: 'ResponseFailed', error: 'API rate limit exceeded' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('error');
    expect(exchangeError(exchanges[0])).toBe('API rate limit exceeded');
  });

  it('streaming buffer shows in last exchange', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Tell me a story' },
      { type: 'TextStreamed', text: 'Once upon a time' },
    ]);
    // Transient text goes to streaming buffer
    handleEvent(map, id, null, { type: 'CumulativeTextUpdated', text: ' there was' });

    const thread = map.get(id)!;
    expect(thread.streamingBuffer).toBe(' there was');

    // Streaming buffer is available via the thread, not the exchange
    // The exchange sees persisted text; buffer is passed to status/rendering
    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(thread.streamingBuffer).toContain('there was');
  });

  it('pendingUserMessages cleared on matching MessageReceived SSE event', () => {
    const { map, id } = makeThread();
    map.get(id)!.pendingUserMessages = [{ text: 'My question', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }];

    // MessageReceived event with matching event_id triggers clearing
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'My question', event_id: 'msg-1' },
    ]);

    expect(map.get(id)!.pendingUserMessages).toEqual([]);

    const thread = map.get(id)!;
    expect(thread.events.size).toBe(1);
  });

  it('pendingUserMessages NOT cleared on non-MessageReceived events', () => {
    const { map, id } = makeThread();
    map.get(id)!.pendingUserMessages = [{ text: 'My question', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }];

    insertEvents(map, id, [
      { type: 'ToolCalled', name: 'search', args: {} },
    ]);

    // Pending message remains — only MessageReceived clears it
    expect(map.get(id)!.pendingUserMessages).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// Flow 2: Follow-up (reply to existing thread)
// ---------------------------------------------------------------------------
describe('Flow: Follow-up', () => {
  it('second MessageReceived creates second exchange', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'First question', created: '2026-01-01T00:00:00Z' },
      { type: 'TextStreamed', text: 'First answer', created: '2026-01-01T00:00:01Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:02Z' },
      { type: 'MessageReceived', text: 'Follow-up', created: '2026-01-01T00:01:00Z' },
      { type: 'TextStreamed', text: 'Follow-up answer', created: '2026-01-01T00:01:01Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:01:02Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // oldest first
    expect(exchangeUserMessage(exchanges[0])).toBe('First question');
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    expect(exchangeUserMessage(exchanges[1])).toBe('Follow-up');
    expect(exchangeStatus(exchanges[1], '', true)).toBe('done');
  });
});

// ---------------------------------------------------------------------------
// Flow 3: Claude Code session
// ---------------------------------------------------------------------------
describe('Flow: Claude Code session', () => {
  it('CC working during active processing', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('coding-agent-working');
    expect(getLabel(exchanges[0])).toBe('Working');
  });

  it('CC idle shows Done (exchange complete — WaitingBanner handles session state)', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'done' },
      { type: 'CodingAgentTextStreamed', text: 'Fixed.' },
      { type: 'ResponseGenerated' },  // emitted BEFORE idle in Claude Code sessions
      { type: 'CodingAgentIdled' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    // Exchange is done — the CC answered. The WaitingBanner (separate component)
    // handles the "CC is idle, you can interact" state.
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    expect(getLabel(exchanges[0])).toBe('Done');
  });

  it('CC SessionEnded shows Done', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'ResponseGenerated' },
      { type: 'SessionEnded' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
  });

  it('CC text shows in response', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentTextStreamed', text: 'I fixed the bug.' },
      { type: 'ResponseGenerated' },
      { type: 'SessionEnded' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeResponseText(exchanges[0])).toContain('I fixed the bug.');
  });
});

// ---------------------------------------------------------------------------
// Flow 4: Scheduled trigger
// ---------------------------------------------------------------------------
describe('Flow: Scheduled trigger', () => {
  it('TriggerStarted creates exchange with trigger channel', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'TriggerStarted', trigger_id: 'task-1' },
      { type: 'ToolCalled', name: 'run_python', args: {} },
      { type: 'ToolResult', name: 'run_python', result: 'ok' },
      { type: 'TextStreamed', text: 'Task done.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeUserChannel(exchanges[0])).toBe('trigger');
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    expect(exchangeResponseText(exchanges[0])).toBe('Task done.');
  });

  it('TriggerStarted with prompt shows the prompt as userMessage', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'TriggerStarted', trigger_id: 'task-1', trigger_name: 'Daily Check', prompt: 'Check my emails and summarize' },
      { type: 'TextStreamed', text: 'All clear.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeUserMessage(exchanges[0])).toBe('Check my emails and summarize');
    expect(exchangeUserChannel(exchanges[0])).toBe('trigger');
  });

  it('TriggerStarted without prompt falls back to trigger_name', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'TriggerStarted', trigger_id: 'task-1', trigger_name: 'Daily Check' },
      { type: 'TextStreamed', text: 'Done.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeUserMessage(exchanges[0])).toBe('Daily Check');
  });

  it('scheduled trigger without completion stays running until backend sends completion event', () => {
    const { map, id } = makeThread();
    const staleTime = new Date(Date.now() - 120_000).toISOString(); // 2 minutes ago

    insertEvents(map, id, [
      { type: 'TriggerStarted', trigger_id: 'task-1', created: staleTime },
      { type: 'ToolCalled', name: 'execute_intent', args: {}, created: staleTime },
      { type: 'ToolResult', name: 'execute_intent', result: 'ok', created: staleTime },
      // No ResponseGenerated/ResponseAborted — TriggerStarted set 'running'
      // and no completion event has arrived yet. Frontend mirrors backend events.
    ]);

    const status = map.get(id)!.meta.status;
    expect(status).toBe('running');
  });
});

// ---------------------------------------------------------------------------
// Message mode / route label
// ---------------------------------------------------------------------------
describe('exchangeUserSource — reads MessageReceived.mode', () => {
  it('human-mode MessageReceived returns "user"', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'hi', mode: 'human' } as any,
    ]);
    const exchanges = getExchanges(map, id);
    expect(exchangeUserSource(exchanges[0])).toBe('user');
  });

  it('agent-mode MessageReceived returns "system"', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'auto', mode: 'agent' } as any,
    ]);
    const exchanges = getExchanges(map, id);
    expect(exchangeUserSource(exchanges[0])).toBe('system');
  });

  it('engine-mode MessageReceived returns "system"', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'auto', mode: 'engine' } as any,
    ]);
    const exchanges = getExchanges(map, id);
    expect(exchangeUserSource(exchanges[0])).toBe('system');
  });

  it('MessageReceived without mode defaults to "user" (mirrors engine default_mode_human for old DB rows)', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [{ type: 'MessageReceived', text: 'hi' }]);
    const exchanges = getExchanges(map, id);
    expect(exchangeUserSource(exchanges[0])).toBe('user');
  });
});

describe('Route label — system-initiated thread, user follow-up', () => {
  it('user follow-up in scheduled-trigger CC thread renders "User → Claude Code"', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'TriggerStarted', trigger_id: 't-1', trigger_name: 'Daily', prompt: 'Run it' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled' },
      { type: 'MessageReceived', text: 'what model is used?', channel: 'claude_code', mode: 'human' } as any,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    expect(exchangeUserSource(exchanges[0])).toBe('system');
    expect(exchangeUserChannel(exchanges[0])).toBe('trigger');

    expect(exchangeUserSource(exchanges[1])).toBe('user');
    expect(exchangeUserChannel(exchanges[1])).toBe('claude_code');
  });
});

// ---------------------------------------------------------------------------
// Flow 5: CC follow-up
// ---------------------------------------------------------------------------
describe('Flow: CC follow-up', () => {
  it('follow-up creates second exchange, first becomes done', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix bug', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-01-01T00:00:03Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:04Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:05Z' },
      // Follow-up — backend emits MessageReceived
      { type: 'MessageReceived', text: 'Also fix tests', created: '2026-01-01T00:01:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:01:01Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-01-01T00:01:02Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:01:03Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:01:04Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // oldest first
    expect(exchangeUserMessage(exchanges[0])).toBe('Fix bug');
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');  // CC went idle → clean completion, not interrupted
    expect(getLabel(exchanges[0], '', false)).toBe('Done');
    expect(exchangeUserMessage(exchanges[1])).toBe('Also fix tests');
    expect(exchangeStatus(exchanges[1], '', true)).toBe('done');  // last exchange, CC idle → done
  });

  it('legacy CodingAgentUserMessageSent creates exchange boundary for old data', () => {
    // Old data has CodingAgentUserMessageSent instead of MessageReceived for CC follow-ups.
    // Frontend must still create a separate exchange, not render inside steps.
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix bug', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-01-01T00:00:03Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:04Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:05Z' },
      // Legacy: only CodingAgentUserMessageSent, no MessageReceived
      { type: 'CodingAgentUserMessageSent', text: 'Also fix tests', created: '2026-01-01T00:01:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:01:01Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-01-01T00:01:02Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:01:03Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:01:04Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // oldest first
    expect(exchangeUserMessage(exchanges[0])).toBe('Fix bug');
    expect(exchangeUserMessage(exchanges[1])).toBe('Also fix tests');
  });

  it('deduplicates MessageReceived + CodingAgentUserMessageSent for same follow-up', () => {
    // New data emits both MessageReceived (from frontend) and CodingAgentUserMessageSent
    // (from backend) for the same follow-up. Should produce ONE exchange, not two.
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix bug', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-01-01T00:00:03Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:04Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:05Z' },
      // Follow-up: both events emitted for the same message
      { type: 'MessageReceived', text: 'Also fix tests', created: '2026-01-01T00:01:00Z' },
      { type: 'CodingAgentUserMessageSent', text: 'Also fix tests', created: '2026-01-01T00:01:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:01:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-01-01T00:01:03Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:01:04Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:01:05Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);  // NOT 3
    expect(exchangeUserMessage(exchanges[0])).toBe('Fix bug');
    expect(exchangeUserMessage(exchanges[1])).toBe('Also fix tests');
    // Steps should be on the second exchange (tools, response, idle)
    expect(exchanges[1].steps.length).toBeGreaterThanOrEqual(4);
  });
});

// ---------------------------------------------------------------------------
// Flow 6: Disconnected message
// ---------------------------------------------------------------------------
describe('Flow: Disconnected message', () => {
  it('MessageReceived + ResponseFailed shows error', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'My message' },
      { type: 'ResponseFailed', error: 'Disconnected from engine' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeUserMessage(exchanges[0])).toBe('My message');
    expect(exchangeStatus(exchanges[0], '', true)).toBe('error');
    expect(exchangeError(exchanges[0])).toBe('Disconnected from engine');
  });
});

// ---------------------------------------------------------------------------
// Flow 7: Thread status
// ---------------------------------------------------------------------------
describe('Flow: Thread status', () => {
  it('empty → idle', () => {
    const { map, id } = makeThread();
    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('ResponseGenerated → idle', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'hi' },
      { type: 'ResponseGenerated' },
    ]);
    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('ToolCalled (no completion) → running (MessageReceived sets status)', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'hi' },
      { type: 'ToolCalled', name: 'search', args: {} },
    ]);
    // MessageReceived event sets meta.status = 'running'
    expect(map.get(id)!.meta.status).toBe('running');
  });

  it('CodingAgentIdled without changes → idle', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'hi' },
      { type: 'CodingAgentIdled' },
    ]);
    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('CodingAgentIdled with has_changes → waiting', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Fixed.' },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);
    expect(map.get(id)!.meta.status).toBe('waiting');
    // has_changes is stored in the event
    const idleEvents = [...map.get(id)!.events.values()].filter(e => e.type === 'CodingAgentIdled');
    expect(idleEvents).toHaveLength(1);
    expect((idleEvents[0] as any).has_changes).toBe(true);
  });

  it('CodingAgentIdled without has_changes defaults to false', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'check something' },
      { type: 'CodingAgentIdled' },
    ]);
    const idleEvents = [...map.get(id)!.events.values()].filter(e => e.type === 'CodingAgentIdled');
    expect(idleEvents).toHaveLength(1);
    // has_changes is undefined/falsy when not present
    expect((idleEvents[0] as any).has_changes).toBeFalsy();
  });

  it('revived CC thread (new message after SessionEnded) → running', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      // First Claude Code session — completed
      { type: 'MessageReceived', text: 'Fix bug', created: t(-60000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-59000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-58000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-57000) },
      { type: 'ResponseGenerated', created: t(-56000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-55000) },
      { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix', files: ['f.rs'], created: t(-54000) },
      { type: 'SessionEnded', created: t(-53000) },
      { type: 'ChangeApplied', change_id: 'c-1', created: t(-52000) },
      // Revived — new message starts a new Claude Code session
      { type: 'MessageReceived', text: 'Now fix tests', created: t(-5000) },
      { type: 'SessionStarted', session_id: 's2', created: t(-4000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-3000) },
    ]);

    // Thread should be running — second MessageReceived set status='running'
    expect(map.get(id)!.meta.status).toBe('running');
  });

  it('revived CC thread that fails immediately → error (not stuck running)', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      // First Claude Code session — completed normally
      { type: 'MessageReceived', text: 'Fix bug', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:01Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentIdled', has_changes: true, created: '2026-01-01T00:00:03Z' },
      { type: 'SessionEnded', created: '2026-01-01T00:00:04Z' },
      // Revived — but CC spawn fails (e.g., "already running")
      { type: 'MessageReceived', text: 'Now fix tests', created: '2026-01-01T00:01:00Z' },
      { type: 'ResponseFailed', error: 'Claude Code is already running for this thread', created: '2026-01-01T00:01:01Z' },
    ]);

    // Thread must be in 'failed' status (error needs user attention, distinct
    // from 'waiting' which means CC has changes to review), NOT stuck in running.
    expect(map.get(id)!.meta.status).toBe('failed');

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeUserMessage(exchanges[1])).toBe('Now fix tests');
    expect(exchangeStatus(exchanges[1], '', true)).toBe('error');
    expect(exchangeError(exchanges[1])).toContain('already running');
  });

  it('revived CC thread shows correct exchange count', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      // First exchange — completed
      { type: 'MessageReceived', text: 'Fix bug', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentTextStreamed', text: 'Fixed.', created: '2026-01-01T00:00:02Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:03Z' },
      { type: 'CodingAgentIdled', has_changes: true, created: '2026-01-01T00:00:04Z' },
      { type: 'SessionEnded', created: '2026-01-01T00:00:05Z' },
      { type: 'ChangeApplied', change_id: 'c-1', created: '2026-01-01T00:00:06Z' },
      // Second exchange — revived, actively running
      { type: 'MessageReceived', text: 'Now fix tests', created: '2026-01-01T00:01:00Z' },
      { type: 'SessionStarted', session_id: 's2', created: '2026-01-01T00:01:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: '2026-01-01T00:01:02Z' },
    ]);

    const exchanges = getExchanges(map, id);
    // 1: CC fix-bug exchange, 2: ChangeApplied initiator panel, 3: CC fix-tests exchange
    expect(exchanges).toHaveLength(3);
    expect(exchangeUserMessage(exchanges[0])).toBe('Fix bug');
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    expect(exchanges[1].userEvent.type).toBe('ChangeApplied');
    expect(exchangeUserMessage(exchanges[2])).toBe('Now fix tests');
    expect(exchangeStatus(exchanges[2], '', true)).toBe('coding-agent-working');
  });

  it('long-running tool call (>60s) stays running (MessageReceived sets status)', () => {
    const { map, id } = makeThread();
    // Simulate: user sent a message, LLM called read_file which is taking >60s
    const twoMinutesAgo = new Date(Date.now() - 120_000).toISOString();
    const almostTwoMinutesAgo = new Date(Date.now() - 119_000).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'read the large file', created: twoMinutesAgo },
      { type: 'ToolCalled', name: 'read_file', args: { path: 'big.txt' }, created: almostTwoMinutesAgo },
    ]);

    // MessageReceived set status='running', no completion event → still running
    expect(map.get(id)!.meta.status).toBe('running');
  });

  it('activity event after completion bumps status back to running', () => {
    // Chat-side mirror of the CC premature-Idled recovery — see
    // thread_lifecycle.rs status_transitions: any activity event proves
    // work is in progress and re-marks the thread Running so it leaves
    // the REVIEW section while streaming continues.
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'hi', created: '2026-01-01T00:00:00Z' },
      { type: 'ToolCalled', name: 'search', args: {}, created: '2026-01-01T00:00:01Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:02Z' },
      { type: 'ToolResult', name: 'search', result: 'found', created: '2026-01-01T00:00:03Z' },
    ]);
    expect(map.get(id)!.meta.status).toBe('running');
  });
});

// ---------------------------------------------------------------------------
// Flow 8: Stale exchange
// ---------------------------------------------------------------------------
describe('Flow: Stale last exchange', () => {
  it('old events with no completion → streaming (backend handles crash detection)', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'old question', created: '2026-01-01T00:00:00Z' },
      { type: 'ToolCalled', name: 'search', args: {}, created: '2026-01-01T00:00:01Z' },
      { type: 'ToolResult', name: 'search', result: 'found', created: '2026-01-01T00:00:02Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    // No stale guard — backend emits ResponseAborted on crash/restart.
    // Without a terminal event, falls through to steps/events check → streaming.
    expect(exchangeStatus(exchanges[0], '', true)).toBe('streaming');
  });
});

// ---------------------------------------------------------------------------
// Flow 9: Chronological ordering
// ---------------------------------------------------------------------------
describe('Flow: Event ordering', () => {
  it('events sort by created timestamp, not sequence', () => {
    const { map, id } = makeThread();

    // Insert with timestamps that don't match sequence order
    handleEvent(map, id, 100, { type: 'MessageReceived', text: 'second' } as ThreadEvent, '2026-01-01T00:01:00Z');
    handleEvent(map, id, 50, { type: 'MessageReceived', text: 'first' } as ThreadEvent, '2026-01-01T00:00:00Z');

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    expect(exchanges).toHaveLength(2);
    expect((exchanges[0].userEvent as any).text).toBe('first');
    expect((exchanges[1].userEvent as any).text).toBe('second');
  });
});

// ---------------------------------------------------------------------------
// Flow 10: More/Less and Steps toggles
// ---------------------------------------------------------------------------
describe('Flow: Toggle visibility', () => {
  it('no steps → no toggles', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Hello' },
      { type: 'TextStreamed', text: 'Hi there!' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    const { showMoreToggle, showStepsToggle } = getEventToggleState(exchangeResponseEvents(exchanges[0]));
    expect(showStepsToggle).toBe(false);
    expect(showMoreToggle).toBe(false);
  });

  it('steps present → showStepsToggle true', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Search for cats' },
      { type: 'ToolCalled', name: 'web_search', args: {} },
      { type: 'ToolResult', name: 'web_search', result: 'found cats' },
      { type: 'TextStreamed', text: 'Here are cats.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    const { showStepsToggle } = getEventToggleState(exchangeResponseEvents(exchanges[0]));
    expect(showStepsToggle).toBe(true);
  });

  it('steps + 2 text blocks → showMoreToggle true', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Multi-step task' },
      { type: 'TextStreamed', text: 'First I will search.' },
      { type: 'ToolCalled', name: 'web_search', args: {} },
      { type: 'ToolResult', name: 'web_search', result: 'results' },
      { type: 'TextStreamed', text: 'Now I will summarize.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    const { showMoreToggle, showStepsToggle } = getEventToggleState(exchangeResponseEvents(exchanges[0]));
    expect(showStepsToggle).toBe(true);
    expect(showMoreToggle).toBe(true);
  });

  it('steps + only 1 text block → showMoreToggle false', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Simple tool use' },
      { type: 'ToolCalled', name: 'calculator', args: {} },
      { type: 'ToolResult', name: 'calculator', result: '42' },
      { type: 'TextStreamed', text: 'The answer is 42.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    const { showMoreToggle, showStepsToggle } = getEventToggleState(exchangeResponseEvents(exchanges[0]));
    expect(showStepsToggle).toBe(true);
    expect(showMoreToggle).toBe(false);
  });

  it('collapsed view shows last text block', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Multi-step' },
      { type: 'TextStreamed', text: 'First part.' },
      { type: 'ToolCalled', name: 'search', args: {} },
      { type: 'ToolResult', name: 'search', result: 'ok' },
      { type: 'TextStreamed', text: 'Final answer.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    const events = exchangeResponseEvents(exchanges[0]);
    const { visibleEvents, needsFallback } = getCollapsedVisibleEvents(events);

    // Collapsed view should show the last meaningful text block
    expect(needsFallback).toBe(false);
    const visibleText = visibleEvents.filter(e => e.type === 'text');
    expect(visibleText.length).toBeGreaterThan(0);
    expect((visibleText[0] as { md: string }).md).toBe('Final answer.');
  });

  it('Claude Code session with tools shows steps toggle', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'file content' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'done' },
      { type: 'CodingAgentTextStreamed', text: 'Fixed the bug.' },
      { type: 'ResponseGenerated' },
      { type: 'SessionEnded' },
    ]);

    const exchanges = getExchanges(map, id);
    const { showStepsToggle } = getEventToggleState(exchangeResponseEvents(exchanges[0]));
    expect(showStepsToggle).toBe(true);

    // Should have CC step events
    const events = exchangeResponseEvents(exchanges[0]);
    const ccSteps = events.filter(e => e.type === 'step');
    expect(ccSteps.length).toBe(2);
    expect((ccSteps[0] as any).description).toBe('Read file');
    expect((ccSteps[1] as any).description).toBe('Edit file');
  });
});

// ---------------------------------------------------------------------------
// Flow 11: Interrupted (Done ↳)
// ---------------------------------------------------------------------------
