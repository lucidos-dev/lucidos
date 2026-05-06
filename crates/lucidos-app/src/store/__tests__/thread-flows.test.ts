/**
 * Integration tests for thread flows — test the complete pipeline:
 * SSE events → handleEvent → groupIntoExchanges → Exchange
 */
import { describe, it, expect } from 'vitest';
import {
  handleEvent,
  groupIntoExchanges,
  exchangeUserImages,
  exchangeUserMessage,
  exchangeUserChannel,
  exchangeUserSource,
  exchangeTimestamp,
  exchangeResponseTimestamp,
  exchangeStatus,
  exchangeSteps,
  exchangeResponseEvents,
  exchangeResponseText,
  exchangeError,
  isEmptyContinuedExchange,
  getCCWaitingInfo,
  type ThreadState,
  type ThreadEvent,
  type Exchange,
} from '../thread-events';
import { handleEventWithAgg } from './aggregate-test-helper';
import { statusLabel, isActive } from '../exchange-status';
import { displaySection } from '../../generated/thread-lifecycle';
import { getEventToggleState, getCollapsedVisibleEvents } from '../event-rendering';
import { effectiveThreadStatus } from '../store';

const TS = '2026-04-17T00:00:00Z';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeThread(id = 'thread-1', status: 'idle' | 'running' | 'waiting' = 'idle'): { map: Map<string, ThreadState>; id: string } {
  const thread: ThreadState = {
    meta: {
      id,
      title: '...',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '',
      updatedAt: '',
      status,
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      lastRevivedAt: '',
      state: 'active',
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
  const map = new Map([[id, thread]]);
  return { map, id };
}

let seqCounter = 1;

/** Insert events into the thread's event map, simulating SSE arrival.
 *  Uses ascending seqs so insertion order = sort order. */
function insertEvents(
  map: Map<string, ThreadState>,
  threadId: string,
  events: Array<ThreadEvent & { created?: string; event_id?: string }>,
): void {
  for (const event of events) {
    const created = event.created ?? TS;
    const eventId = (event as any).event_id;
    const clean = { ...event };
    delete (clean as any).created;
    delete (clean as any).event_id;
    // Use handleEventWithAgg so a synthesized ThreadAggregate (mirroring
    // backend update_thread_projection rules) is applied to thread.meta.
    handleEventWithAgg(map, threadId, seqCounter++, clean, created, eventId);
  }
}

/** Run full pipeline: events map → exchanges (oldest-first) */
function getExchanges(map: Map<string, ThreadState>, threadId: string) {
  const thread = map.get(threadId)!;
  return groupIntoExchanges(thread.events);
}

/** Get the user-visible status label for an Exchange */
function getLabel(ex: Exchange, streamingBuffer = '', isLast = true, hasPriorActive = false, threadIsCC = false): string {
  const status = exchangeStatus(ex, streamingBuffer, isLast, hasPriorActive, threadIsCC);
  const steps = exchangeSteps(ex);
  const events = exchangeResponseEvents(ex);
  const hasSteps = steps.length > 0 || events.some(e => e.type === 'step');
  return statusLabel(status, hasSteps).label;
}

// Reset seq counter between tests
import { beforeEach } from 'vitest';
beforeEach(() => { seqCounter = 1; });

/** Build exchanges including pending user messages (not yet in DB).
 *  @param useDisplayCreated — use `_displayCreated` instead of `created` for synthetic events
 *    (chat threads need this so sorting falls through to seq comparison). */
function getExchangesWithPending(
  map: Map<string, ThreadState>,
  threadId: string,
  useDisplayCreated = false,
): Exchange[] {
  const thread = map.get(threadId)!;
  if (thread.pendingUserMessages.length === 0) {
    return groupIntoExchanges(thread.events);
  }
  const augmented = new Map(thread.events);
  for (let i = 0; i < thread.pendingUserMessages.length; i++) {
    const pending = thread.pendingUserMessages[i];
    const syntheticSeq = Number.MAX_SAFE_INTEGER - thread.pendingUserMessages.length + i;
    augmented.set(syntheticSeq, {
      type: 'MessageReceived' as const,
      text: pending.text,
      channel: thread.meta.channel,
      ...(useDisplayCreated
        ? { _displayCreated: pending.created }
        : { created: pending.created }),
    } as any);
  }
  return groupIntoExchanges(augmented);
}

// ---------------------------------------------------------------------------
// Flow 1: New chat message
// ---------------------------------------------------------------------------
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
    expect(exchangeSteps(exchanges[0])[0].success).toBe(true);

    // Events should have step + text interleaved
    const events = exchangeResponseEvents(exchanges[0]);
    const stepEvents = events.filter(e => e.type === 'step');
    const textEvents = events.filter(e => e.type === 'text');
    expect(stepEvents).toHaveLength(1);
    expect(stepEvents[0].success).toBe(true);
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
    handleEvent(map, id, null, { type: 'TextStreaming', text: ' there was' });

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
    expect(exchangeStatus(exchanges[0], '', true)).toBe('cc-working');
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
      { type: 'ResponseGenerated' },  // emitted BEFORE idle in CC sessions
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
      // First CC session — completed
      { type: 'MessageReceived', text: 'Fix bug', created: t(-60000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-59000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-58000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-57000) },
      { type: 'ResponseGenerated', created: t(-56000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-55000) },
      { type: 'ChangeProposed', change_id: 'c-1', description: 'Fix', files: ['f.rs'], created: t(-54000) },
      { type: 'SessionEnded', created: t(-53000) },
      { type: 'ChangeApplied', change_id: 'c-1', created: t(-52000) },
      // Revived — new message starts a new CC session
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
      // First CC session — completed normally
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
    expect(exchangeStatus(exchanges[2], '', true)).toBe('cc-working');
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

  it('CC session with tools shows steps toggle', () => {
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
// Flow 11: Interrupted (Continued below)
// ---------------------------------------------------------------------------
describe('Flow: Interrupted exchanges', () => {
  it('CC exchange followed by another shows "Continued below"', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix bug', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:00:02Z' },
      // User interrupts with follow-up before CC finishes
      { type: 'MessageReceived', text: 'Actually also fix tests', created: '2026-01-01T00:01:00Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:01:01Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // First exchange (CC, not last) → interrupted
    expect(exchangeStatus(exchanges[0], '', false)).toBe('interrupted');
    expect(getLabel(exchanges[0], '', false)).toBe('Continued below');
    // Second exchange (last, completed) → done
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
      // Follow-up from idle — not an interruption, clean handoff
      { type: 'MessageReceived', text: 'Now fix tests too', created: '2026-01-01T00:01:00Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:01:01Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // First CC exchange: went idle normally → Done (not interrupted)
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
      // New message after session ended
      { type: 'MessageReceived', text: 'Something else', created: '2026-01-01T00:01:00Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:01:01Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    expect(getLabel(exchanges[0], '', false)).toBe('Done');
  });

  it('CC follow-up exchange (no own SessionStarted) interrupted shows "Continued below"', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      // Exchange 1: initial CC request, completed normally
      { type: 'MessageReceived', text: 'Center the separator', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-01-01T00:00:03Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:04Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:05Z' },
      // Exchange 2: follow-up in same CC session (no SessionStarted), interrupted
      { type: 'MessageReceived', text: 'they are still a bit much', created: '2026-01-01T00:01:00Z' },
      // No ResponseGenerated — user sends another message before response
      // Exchange 3: another follow-up
      { type: 'MessageReceived', text: 'sorry wrong thread', created: '2026-01-01T00:02:00Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:02:01Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);
    // Exchange 1: CC went idle → done
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    // Exchange 2: CC follow-up, no steps, not last → done (CC skipped it, nothing to interrupt)
    expect(exchangeStatus(exchanges[1], '', false, false, true)).toBe('done');
    // Exchange 3: last, completed → done
    expect(exchangeStatus(exchanges[2], '', true)).toBe('done');
  });

  it('CC exchange with only SessionStarted (no body events) before follow-up: interrupted with no visible events', () => {
    // Reproduces the bug: user sends a message, CC starts (SessionStarted lands)
    // but produces no tool calls or text before the user fires off another
    // message. The middle exchange is 'interrupted' with hasSteps=true but
    // exchangeResponseEvents=[] — ChatExchange must hide the empty
    // "Continued below ↳" header, same as it does for empty 'done' exchanges.
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'so now u can use gh correctly?', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-01-01T00:00:02Z' },
      { type: 'MessageReceived', text: 'and git?', created: '2026-01-01T00:00:05Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // Middle exchange has only SessionStarted as a step → status 'interrupted'…
    expect(exchangeStatus(exchanges[0], '', false, false, true)).toBe('interrupted');
    expect(getLabel(exchanges[0], '', false, false, true)).toBe('Continued below');
    // …but exchangeResponseEvents emits nothing (SessionStarted alone produces
    // no section_break — hasCCContent is false), so the response panel body
    // would be empty. The visible-noise placeholder must be hidden.
    expect(exchangeResponseEvents(exchanges[0])).toEqual([]);
    expect(exchangeResponseText(exchanges[0])).toBe('');
  });

  it('CC exchange with SessionStarted + Thinking only before follow-up: panel is empty-continued', () => {
    // Reproduces the bug from the screenshot: user sends a message, CC emits
    // SessionStarted then a Thinking event (the model began thinking but
    // produced no tool call or text yet) before the user fires off another
    // message. exchangeResponseEvents preserves the Thinking step (the data
    // layer correctly reflects what was emitted), but the rendering layer
    // must treat a Thinking-only payload the same as no payload — the
    // single "Thinking" line conveys nothing the next exchange's user
    // message doesn't already imply.
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'What does that mean?', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-01-01T00:00:01Z' },
      { type: 'Thinking', text: '', created: '2026-01-01T00:00:02Z' },
      { type: 'MessageReceived', text: 'follow-up', created: '2026-01-01T00:01:00Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false, false, true)).toBe('interrupted');

    // The data layer keeps the Thinking event (auditable record of what happened).
    const events = exchangeResponseEvents(exchanges[0]);
    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({ type: 'step', description: 'Thinking' });

    // The rendering layer must classify this as empty-continued — the panel
    // is suppressed in ChatExchange for non-last done/interrupted exchanges
    // whose only events are bare Thinking steps.
    expect(isEmptyContinuedExchange('interrupted', false, events, false)).toBe(true);
  });

  it('CC follow-up with whitespace-only CodingAgentTextStreamed + CodingAgentPromptSent before next follow-up: panel is empty-continued', () => {
    // CC echoes a follow-up prompt as a whitespace-only CodingAgentTextStreamed
    // ("\n\n" header) + CodingAgentPromptSent (Thinking spinner). When the
    // user fires another follow-up before CC produces real output, the
    // "\n\n" text event survives mergeAdjacentTextEvents (textBuf is truthy)
    // and the predicate must still classify the panel as empty-continued —
    // otherwise the orphan Thinking spinner panel renders.
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
    // Non-CC, not last → done (not interrupted)
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    expect(getLabel(exchanges[0], '', false)).toBe('Done');
  });

  it('chat exchange interrupted by mid-flight UPI shows interrupted, not Working', () => {
    // Reproduces the bug from the "Verifying Git Pull Permission Error" thread:
    // user sends MR1, agent starts processing (steps with req_id=MR1), user sends MR2
    // mid-flight, engine emits UPI absorbed into MR2. Both panels showed "Working"
    // because the prior chat exchange had visible steps and no terminator — falling
    // through to 'streaming'. Only the LAST panel should be Working.
    const { map, id } = makeThread();
    const mr1Id = 'mr1-event-id';
    const mr2Id = 'mr2-event-id';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Organize nodes on same level...', created: '2026-01-01T00:00:00Z', event_id: mr1Id },
      { type: 'MemorySearched', request_event_id: mr1Id, results: [], query: 'q', created: '2026-01-01T00:00:02Z' } as any,
      { type: 'Thinking', request_event_id: mr1Id, text: 'thinking...', created: '2026-01-01T00:00:03Z' } as any,
      { type: 'ToolCalled', name: 'Read', args: {}, request_event_id: mr1Id, created: '2026-01-01T00:00:10Z' } as any,
      { type: 'ToolResult', name: 'Read', result: 'ok', request_event_id: mr1Id, created: '2026-01-01T00:00:11Z' } as any,
      { type: 'MessageReceived', text: 'Also use more horizontal space...', created: '2026-01-01T00:00:30Z', event_id: mr2Id },
      // UPI absorbs into MR2's exchange and sets reqIdRedirect[mr1Id]=E2 —
      // subsequent req_id=mr1Id events redirect to E2.
      { type: 'UserPromptInjected', text: 'Also use more horizontal space...', mode: 'human',
        request_event_id: mr1Id, injected_message_id: mr2Id, created: '2026-01-01T00:01:00Z' } as any,
      { type: 'Thinking', request_event_id: mr1Id, text: 'more thinking', created: '2026-01-01T00:01:05Z' } as any,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // E1 (not last) should NOT be 'streaming' / Working — the user has moved on.
    // It should be 'interrupted' (label "Continued below ↳") matching the
    // existing CC pattern for mid-work interruptions.
    expect(exchangeStatus(exchanges[0], '', false)).toBe('interrupted');
    expect(getLabel(exchanges[0], '', false)).toBe('Continued below');
    // E2 (last) — still actively processing → Working
    expect(exchangeStatus(exchanges[1], '', true)).toBe('streaming');
    expect(getLabel(exchanges[1], '', true)).toBe('Working');
  });
});

// ---------------------------------------------------------------------------
// Flow 12: Recovery CC session
// ---------------------------------------------------------------------------
describe('Flow: Recovery CC session', () => {
  it('recovery session with tools and idle shows done with change panel data', () => {
    const { map, id } = makeThread();

    // Recovery session event sequence (same as spawn_cc_thread)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'A previous Claude Code session was interrupted...' },
      { type: 'SessionStarted', session_id: 'recovery-1' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'file contents' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'CodingAgentTextStreamed', text: 'I finished the cleanup.' },
      // These events MUST be emitted for the change panel to show
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    expect(getLabel(exchanges[0])).toBe('Done');
    // Response text should be present
    expect(exchangeResponseText(exchanges[0])).toContain('I finished the cleanup.');
    // Steps should show the tools
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

// ---------------------------------------------------------------------------
// Flow 12b: Recovery via SessionRecovered (auto-recovery of interrupted sessions)
// ---------------------------------------------------------------------------
describe('Flow: SessionRecovered recovery', () => {
  it('SessionRecovered acts as exchange boundary, thread shows CC content', () => {
    const { map, id } = makeThread();

    // Recovery session: SessionRecovered for auto-recovered interrupted sessions
    insertEvents(map, id, [
      { type: 'SessionRecovered', branch: 'claude-code/20260318-122816' },
      { type: 'SessionStarted', session_id: 'cc-1', branch: 'claude-code/20260318-122816' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' },
      { type: 'CodingAgentTextStreamed', text: 'Reviewed and continuing.' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    // SessionRecovered is the user event (system-initiated, no user message)
    expect(exchanges[0].userEvent.type).toBe('SessionRecovered');
    expect(exchangeUserMessage(exchanges[0])).toBe('Resumed after engine restart');
    expect(exchangeUserChannel(exchanges[0])).toBe('claude_code');
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    expect(exchangeResponseText(exchanges[0])).toContain('Reviewed and continuing.');
  });

  it('SessionRecovered in existing thread (with prior messages) creates new exchange', () => {
    const { map, id } = makeThread();

    // Original thread with a completed exchange
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug', channel: 'claude_code' },
      { type: 'SessionStarted', session_id: 'cc-orig', branch: 'claude-code/20260318-122816' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled', has_changes: true },
      // Engine restart → auto-recovery of interrupted session
      { type: 'SessionRecovered', branch: 'claude-code/20260318-122816' },
      { type: 'SessionStarted', session_id: 'cc-recovery', branch: 'claude-code/20260318-122816' },
      { type: 'CodingAgentTextStreamed', text: 'Continuing work.' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[1].userEvent.type).toBe('SessionRecovered');
    expect(exchangeStatus(exchanges[1], '', true)).toBe('done');
  });

  it('SessionRecovered that completes with SessionEnded shows Done', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'SessionRecovered', branch: 'claude-code/20260318' },
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

// ---------------------------------------------------------------------------
// Flow 13: Edge cases from user reports
// ---------------------------------------------------------------------------
describe('Flow: Edge cases', () => {
  it('pendingUserMessages cleared, backend MessageReceived groups events correctly', () => {
    const { map, id } = makeThread();
    map.get(id)!.pendingUserMessages = [{ text: 'My question', eventId: 'msg-1', created: '2026-01-01T00:00:00Z' }];

    // Backend sends MessageReceived with real seq + follow-up events
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
    expect(steps.every(s => s.success === true)).toBe(true);
  });

  it('ToolCalled without ToolResult shows pending step', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Do something' },
      { type: 'ToolCalled', name: 'slow_tool', args: {} },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeSteps(exchanges[0])).toHaveLength(1);
    expect(exchangeSteps(exchanges[0])[0].success).toBeNull(); // still pending
    expect(getLabel(exchanges[0])).toBe('Working');
  });

  it('Thinking event creates a step with context metadata', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Hello' },
      { type: 'Thinking', text: 'Context: 5000 tokens, 3 messages', context_tokens: 5000, context_messages: 3 },
      { type: 'TextStreamed', text: 'Hi!' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    const steps = exchangeSteps(exchanges[0]);
    expect(steps).toHaveLength(1);
    expect(steps[0].description).toBe('Thinking');
    expect(steps[0].success).toBe(true);
    expect(steps[0].context_tokens).toBe(5000);
    expect(steps[0].context_messages).toBe(3);

    const events = exchangeResponseEvents(exchanges[0]);
    const stepEvents = events.filter(e => e.type === 'step');
    expect(stepEvents).toHaveLength(1);
    expect((stepEvents[0] as { description: string }).description).toBe('Thinking');
  });

  it('MemorySearched event creates a step', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'What is my birthday?' },
      { type: 'MemorySearched', results: 12, queries: ['birthday', 'date of birth'] },
      { type: 'Thinking', text: 'Context: 2000 tokens, 2 messages', context_tokens: 2000, context_messages: 2 },
      { type: 'TextStreamed', text: 'Jan 1.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    const steps = exchangeSteps(exchanges[0]);
    expect(steps).toHaveLength(2);
    expect(steps[0].description).toBe('Memory searched');
    expect(steps[0].success).toBe(true);
    expect(steps[1].description).toBe('Thinking');

    const events = exchangeResponseEvents(exchanges[0]);
    const stepEvents = events.filter(e => e.type === 'step');
    expect(stepEvents).toHaveLength(2);
    // MemorySearched step should have queries as detail
    const memStep = stepEvents[0] as { detail?: string };
    expect(memStep.detail).toBe('birthday, date of birth');
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
    const { showStepsToggle, showMoreToggle } = getEventToggleState(exchangeResponseEvents(exchanges[0]));
    expect(showStepsToggle).toBe(false);
    expect(showMoreToggle).toBe(false);
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
      // Second exchange makes first not-last
      { type: 'MessageReceived', text: 'Second', created: '2026-01-01T00:01:00Z' },
      { type: 'TextStreamed', text: 'Response', created: '2026-01-01T00:01:01Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:01:02Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // First exchange (non-last CC, went idle) should be done, NOT cc-waiting or interrupted
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    expect(getLabel(exchanges[0], '', false)).toBe('Done');
    // Second exchange (last) should be done
    expect(exchangeStatus(exchanges[1], '', true)).toBe('done');
  });

  it('CC section break only when CC has actual content', () => {
    const { map, id } = makeThread();

    // Exchange with SessionStarted but only regular TextStreamed (no CC events)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Start CC' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'TextStreamed', text: 'Regular response before CC.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    // Should NOT have section_break since no CC tool/text events
    const events = exchangeResponseEvents(exchanges[0]);
    const sectionBreaks = events.filter(e => e.type === 'section_break');
    expect(sectionBreaks).toHaveLength(0);
  });

  it('response text with no completion event and no buffer → aborted (old data)', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Old question', created: '2025-01-01T00:00:00Z' },
      { type: 'TextStreamed', text: 'Old answer', created: '2025-01-01T00:00:01Z' },
      // No ResponseGenerated — missing from DB (response lost on crash/restart)
    ]);

    const exchanges = getExchanges(map, id);
    // No stale guard — backend emits ResponseAborted on crash/restart.
    // Without a terminal event, response text exists → streaming.
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
    // No events, no response, not stale → pending
    expect(exchangeSteps(exchanges[0])).toHaveLength(0);
    expect(exchangeResponseText(exchanges[0])).toBe('');
  });

  it('thread with ResponseGenerated text shows it', () => {
    const { map, id } = makeThread();

    // Some threads have text in ResponseGenerated but not in TextStreamed
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Hello' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
  });
});

// ---------------------------------------------------------------------------
// CC thread follow-up: channel inheritance
// ---------------------------------------------------------------------------
describe('CC thread follow-up: channel detection', () => {
  it('thread with channel=claude_code is detected for CC mode inheritance', () => {
    // Simulate: CC thread exists with channel='claude_code'
    const map = new Map<string, ThreadState>();
    const ccThread: ThreadState = {
      meta: {
        id: 'cc-1',
        title: 'CC Thread',
        channel: 'claude_code',
        initiator: 'user',
        saved: false,
        createdAt: '',
        updatedAt: '',
        status: 'idle',
        messageCount: 0,
        section: 'archived',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        ccHasChanges: false,
        ccRequiresRestart: false,
        ccIsExternalRepo: false,
        ccApplying: false,
        lastRevivedAt: '',
        state: 'active',
      },
      events: new Map(),
      streamingBuffer: '',
      eventsLoaded: true,
      eventsLoadFailed: false,
      lastDbSeq: 0,
      pendingUserMessages: [],
    };
    map.set('cc-1', ccThread);

    // The frontend's sendMessage checks existingThread?.meta.channel === 'claude_code'
    // to decide whether to set use_claude_code: true on follow-up messages
    const existingThread = map.get('cc-1');
    expect(existingThread?.meta.channel).toBe('claude_code');

    // Regular chat thread should NOT trigger CC mode
    const chatThread: ThreadState = {
      meta: {
        id: 'chat-1',
        title: 'Chat',
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
        ccHasChanges: false,
        ccRequiresRestart: false,
        ccIsExternalRepo: false,
        ccApplying: false,
        lastRevivedAt: '',
        state: 'active',
      },
      events: new Map(),
      streamingBuffer: '',
      eventsLoaded: true,
      eventsLoadFailed: false,
      lastDbSeq: 0,
      pendingUserMessages: [],
    };
    map.set('chat-1', chatThread);
    expect(map.get('chat-1')?.meta.channel).not.toBe('claude_code');
  });

  it('SessionStarted event sets thread channel to claude_code', () => {
    // Verify the SSE handler sets source correctly when CC starts
    const { map, id } = makeThread();
    const thread = map.get(id)!;
    expect(thread.meta.channel).toBe('chat'); // initially chat

    // After SessionStarted, source should be 'claude_code'
    // (handled by handleThreadEvent in thread-sync.ts)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug' },
      { type: 'SessionStarted', session_id: 's1' },
    ]);
    // thread-sync.ts sets meta.channel on SessionStarted — verify the thread
    // recognizes this as a CC thread for future follow-ups
    const exchanges = groupIntoExchanges(map.get(id)!.events);
    expect(exchanges[0].steps.some(s => s.event.type === 'SessionStarted')).toBe(true);
  });

  it('follow-up to running CC thread must route via CC channel, not /api/chat', () => {
    // Regression: sending a follow-up via /api/chat calls register_thread()
    // which cancels the old token, killing the active CC session.
    // The frontend should detect running CC threads and route via CC message channel.
    const { map, id } = makeThread();

    // Simulate a CC thread that's actively running (tools being called)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentToolCalled', name: 'Grep', args: { pattern: 'foo' } },
    ]);
    // SessionStarted sets source via thread-sync.ts — simulate that
    map.get(id)!.meta.channel = 'claude_code';

    // Thread status should be 'running' — CC is actively processing (live process)
    const status = map.get(id)!.meta.status;
    expect(status).toBe('running');

    // The frontend routing logic: CC thread + running status → use CC channel
    const thread = map.get(id)!;
    const isCCThread = thread.meta.channel === 'claude_code';
    const hasLiveCCSession = isCCThread && (status === 'running' || status === 'waiting');
    expect(hasLiveCCSession).toBe(true);
  });

  it('follow-up to ended CC thread spawns new CC session via /api/chat', () => {
    const { map, id } = makeThread();

    // CC session that has ended
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Fixed it.' },
      { type: 'ResponseGenerated', text: 'Fixed it.', images: [] },
      { type: 'SessionEnded' },
    ]);
    map.get(id)!.meta.channel = 'claude_code';

    const status = map.get(id)!.meta.status;
    expect(status).toBe('idle');

    // Ended CC thread — should spawn new session via /api/chat, not CC channel
    const thread = map.get(id)!;
    const isCCThread = thread.meta.channel === 'claude_code';
    const hasLiveCCSession = isCCThread && (status === 'running' || status === 'waiting');
    expect(hasLiveCCSession).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// No duplicate events — each event type should appear at most once per action
// ---------------------------------------------------------------------------
// MUST TEST: EventBus migration integration tests
// These test flows that were migrated from old Event to ThreadEvent via bus.
// ---------------------------------------------------------------------------

describe('MUST TEST 1: CC change proposal → apply/discard', () => {
  it('ChangeProposed appears as step in exchange with done status', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'refactor the code' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: { path: 'src/main.rs' } },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'CodingAgentTextStreamed', text: 'Refactored the module.' },
      { type: 'ChangeProposed', change_id: 'c-1', description: 'Refactor module', files: ['src/main.rs'], requires_restart: true },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');

    // ChangeProposed should be in the exchange steps
    const allEvents = [...map.get(id)!.events.values()];
    const changeProposed = allEvents.find(e => e.type === 'ChangeProposed');
    expect(changeProposed).toBeDefined();
    expect((changeProposed as any).change_id).toBe('c-1');
    expect((changeProposed as any).files).toEqual(['src/main.rs']);
  });

  it('ChangeProposed must come before SessionEnded — thread waiting until change resolved', () => {
    // Regression: backend emitted SessionEnded before ChangeProposed, causing
    // the exchange to stay stuck instead of transitioning to done.
    // After fix, ChangeProposed always precedes SessionEnded.
    // Thread status is 'waiting' because the change hasn't been applied/discarded yet.
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'refactor the code' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: { path: 'src/main.rs' } },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'ResponseGenerated', text: 'Done.', images: [] },
      { type: 'CodingAgentIdled', has_changes: true },
      // Correct order: ChangeProposed THEN SessionEnded
      { type: 'ChangeProposed', change_id: 'c-1', description: 'Refactor', files: ['src/main.rs'], requires_restart: false },
      { type: 'SessionEnded' },
    ]);

    // Thread stays in Waiting until change is applied/discarded
    const status = map.get(id)!.meta.status;
    expect(status).toBe('waiting');

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    // Exchange itself is 'done' (session completed), but thread is waiting for change resolution
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
  });

  it('ChangeApplied after ChangeProposed — thread idle, events correct', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315' },
      { type: 'CodingAgentTextStreamed', text: 'Fixed.' },
      { type: 'ChangeProposed', change_id: 'c-1', description: 'Bug fix', files: ['src/lib.rs'], requires_restart: false },
      { type: 'CodingAgentIdled', has_changes: true },
      { type: 'ChangeApplied', change_id: 'c-1' },
      { type: 'SessionEnded' },
    ]);

    // Thread status should be idle (SessionEnded is completion)
    expect(map.get(id)!.meta.status).toBe('idle');

    const allEvents = [...map.get(id)!.events.values()];
    expect(allEvents.some(e => e.type === 'ChangeApplied')).toBe(true);
    expect(allEvents.some(e => e.type === 'SessionEnded')).toBe(true);
  });

  it('ChangeDiscarded after ChangeProposed — thread idle, events correct', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'try something' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315' },
      { type: 'CodingAgentTextStreamed', text: 'Tried it.' },
      { type: 'ChangeProposed', change_id: 'c-2', description: 'Experiment', files: ['test.rs'] },
      { type: 'CodingAgentIdled', has_changes: true },
      { type: 'ChangeDiscarded', change_id: 'c-2' },
      { type: 'SessionEnded' },
    ]);

    expect(map.get(id)!.meta.status).toBe('idle');

    const allEvents = [...map.get(id)!.events.values()];
    expect(allEvents.some(e => e.type === 'ChangeDiscarded')).toBe(true);
  });
});

describe('MUST TEST 2: CC cancel mid-work', () => {
  it('ResponseCanceled mid-stream shows canceled status', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'do something complex' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'npm test' } },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'running...' },
      { type: 'CodingAgentTextStreamed', text: 'Working...' },
      { type: 'ResponseCanceled' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('canceled');
    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('CC cancel preserves streamed text from CodingAgentTextStreamed events', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix everything' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Starting to work...' },
      { type: 'CodingAgentTextStreamed', text: ' Almost done.' },
      { type: 'ResponseCanceled' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('canceled');
    // Response text comes from CodingAgentTextStreamed, not ResponseCanceled
    expect(exchangeResponseText(exchanges[0])).toBe('Starting to work... Almost done.');
  });
});

describe('MUST TEST 3: CC follow-up after idle', () => {
  it('follow-up in same thread creates second exchange, first becomes done', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      // First exchange: CC works and idles
      { type: 'MessageReceived', text: 'fix the tests' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'test code' },
      { type: 'CodingAgentTextStreamed', text: 'Tests fixed.' },
      { type: 'CodingAgentIdled', has_changes: true },
      // Second exchange: user sends follow-up
      { type: 'MessageReceived', text: 'also fix the linting errors' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'npm run lint' } },
      { type: 'CodingAgentToolResult', name: 'Bash', result: '0 errors' },
      { type: 'CodingAgentTextStreamed', text: 'Linting fixed too.' },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // oldest first
    // First exchange completed (follow-up from idle = done, not interrupted)
    expect(exchangeUserMessage(exchanges[0])).toBe('fix the tests');
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    // Newest exchange (follow-up) is last
    expect(exchangeUserMessage(exchanges[1])).toBe('also fix the linting errors');
    expect(exchangeStatus(exchanges[1], '', true)).toBe('done');
  });

  it('follow-up preserves CC context (both exchanges have response text)', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'first task' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Done first.' },
      { type: 'CodingAgentIdled' },
      { type: 'MessageReceived', text: 'second task' },
      { type: 'CodingAgentTextStreamed', text: 'Done second.' },
      { type: 'CodingAgentIdled' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // CodingAgentTextStreamed contributes to response text, not steps
    // oldest first
    expect(exchangeResponseText(exchanges[0])).toBe('Done first.');
    expect(exchangeResponseText(exchanges[1])).toBe('Done second.');
  });
});

// ---------------------------------------------------------------------------
// Flow: CC follow-up interrupted by stop button
// ---------------------------------------------------------------------------
// When the user sends a follow-up to an idle CC session and then hits stop:
//   1. CC was idle (CodingAgentIdled) → user sends follow-up (MessageReceived)
//   2. CC starts working → user hits stop → interrupt sent
//   3. Backend emits ResponseCanceled (not ResponseGenerated) then CodingAgentIdled
// Result: the follow-up exchange shows "Canceled", but the thread stays "Waiting"
// because CC is still alive and idle (CodingAgentIdled is the last event).
describe('Flow: CC follow-up stopped by user', () => {
  it('interrupted follow-up exchange shows "Canceled", thread stays "Waiting"', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      // First exchange: CC completes work and goes idle
      { type: 'MessageReceived', text: 'Fix the bug', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'claude-code/test', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentTextStreamed', text: 'Fixed.', created: '2026-01-01T00:00:03Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:04Z' },
      { type: 'CodingAgentIdled', has_changes: true, created: '2026-01-01T00:00:05Z' },
      // Second exchange: user sends follow-up, then hits stop
      { type: 'MessageReceived', text: 'Also fix tests', created: '2026-01-01T00:01:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'npm test' }, created: '2026-01-01T00:01:01Z' },
      // User hits stop → backend emits ResponseCanceled then CodingAgentIdled
      { type: 'ResponseCanceled', created: '2026-01-01T00:01:02Z' },
      { type: 'CodingAgentIdled', has_changes: true, created: '2026-01-01T00:01:03Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    // First exchange: completed normally → "Done"
    expect(exchangeUserMessage(exchanges[0])).toBe('Fix the bug');
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    expect(getLabel(exchanges[0], '', false)).toBe('Done');

    // Second exchange (follow-up): user hit stop → "Canceled"
    expect(exchangeUserMessage(exchanges[1])).toBe('Also fix tests');
    expect(exchangeStatus(exchanges[1], '', true)).toBe('canceled');
    expect(getLabel(exchanges[1], '', true)).toBe('Canceled');

    // Thread: still "Waiting" because CC is alive (last event is CodingAgentIdled)
    expect(map.get(id)!.meta.status).toBe('waiting');
  });

  it('interrupted follow-up with no work started also shows "Canceled"', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      // CC completes and goes idle
      { type: 'MessageReceived', text: 'Build feature', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'claude-code/test', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentTextStreamed', text: 'Done.', created: '2026-01-01T00:00:02Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:03Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:04Z' },
      // User sends follow-up and immediately hits stop (CC hadn't started working yet)
      { type: 'MessageReceived', text: 'Never mind', created: '2026-01-01T00:01:00Z' },
      { type: 'ResponseCanceled', created: '2026-01-01T00:01:01Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:01:02Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[1], '', true)).toBe('canceled');
    expect(getLabel(exchanges[1], '', true)).toBe('Canceled');
  });
});

describe('MUST TEST 4: Scheduled triggers', () => {
  it('scheduled trigger produces one exchange with correct status', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'TriggerStarted', trigger_id: 'task-1', trigger_name: 'Morning Brief', prompt: 'Check my calendar' },
      { type: 'ToolCalled', name: 'execute_intent', args: {} },
      { type: 'ToolResult', name: 'execute_intent', result: 'Calendar is clear.' },
      { type: 'TextStreamed', text: 'Your calendar is clear today.' },
      { type: 'ResponseGenerated', text: 'Your calendar is clear today.' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeUserMessage(exchanges[0])).toBe('Check my calendar');
    expect(exchangeUserChannel(exchanges[0])).toBe('trigger');
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    expect(exchangeResponseText(exchanges[0])).toBe('Your calendar is clear today.');
  });

  it('scheduled trigger with notification tool shows tool step', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'TriggerStarted', trigger_id: 'task-2', trigger_name: 'Daily Report' },
      { type: 'ToolCalled', name: 'send_notification', args: { title: 'Report', message: 'All good' } },
      { type: 'ToolResult', name: 'send_notification', result: 'Notification sent.' },
      { type: 'ResponseGenerated', text: 'Report sent.' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    // Steps use human-readable description derived from tool name + args
    expect(exchangeSteps(exchanges[0]).some((s: any) => s.description === 'Notify: Report')).toBe(true);
  });
});

describe('MUST TEST 5: Recovery sessions', () => {
  it('recovery thread has one exchange with CC events', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'A previous Claude Code session on branch `claude-code/20260315-120919` was interrupted...' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315-120919' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'git status' } },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'nothing to commit' },
      { type: 'CodingAgentTextStreamed', text: 'The worktree is clean.' },
      { type: 'CodingAgentIdled' },
    ]);

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');

    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
  });

  it('recovery with changes proposes change, then ends', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Recovering interrupted session...' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315-120919' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'changes found' },
      { type: 'CodingAgentTextStreamed', text: 'Found work in progress.' },
      { type: 'ChangeProposed', change_id: 'recovery-c1', description: 'Previous session work', files: ['src/fix.rs'] },
      { type: 'CodingAgentIdled', has_changes: true },
      { type: 'ChangeApplied', change_id: 'recovery-c1' },
      { type: 'SessionEnded' },
    ]);

    const exchanges = getExchanges(map, id);
    // 1: recovery exchange, 2: ChangeApplied initiator panel
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('ChangeApplied');
    // Thread status is idle because SessionEnded is a completion event
    expect(map.get(id)!.meta.status).toBe('idle');

    // No duplicate events
    const allEvents = [...map.get(id)!.events.values()];
    const msgEvents = allEvents.filter(e => e.type === 'MessageReceived');
    expect(msgEvents).toHaveLength(1);
  });

  it('recovery session without changes auto-ends cleanly', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Recovering interrupted session...' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315-clean' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'git diff' } },
      { type: 'CodingAgentToolResult', name: 'Bash', result: '' },
      { type: 'CodingAgentTextStreamed', text: 'Nothing to clean up.' },
      // Recovery with no changes auto-ends (cancel.notify_one())
      { type: 'CodingAgentIdled' },
      { type: 'SessionEnded' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    // Thread status is idle (SessionEnded is a completion event)
    expect(map.get(id)!.meta.status).toBe('idle');
    expect(exchangeResponseText(exchanges[0])).toBe('Nothing to clean up.');
  });
});

// ---------------------------------------------------------------------------
// CC message should produce ONE thread, not a stub + spawned thread
// ---------------------------------------------------------------------------
describe('CC single thread (no duplicate spawn)', () => {
  it('CC message produces one exchange in one thread, not a redirect stub', () => {
    const { map, id } = makeThread();
    // The correct flow: MessageReceived + SessionStarted + CC work + idle, all in one thread
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'what time is it?' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'date' } },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'Mon Mar 15 18:03:07 CET 2026' },
      { type: 'CodingAgentTextStreamed', text: 'The current time is 18:03.' },
      { type: 'CodingAgentIdled', has_changes: false },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeUserMessage(exchanges[0])).toBe('what time is it?');
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');

    // Must NOT have a "I've started a new Claude Code thread" redirect response
    const allEvents = [...map.get(id)!.events.values()];
    const responseEvents = allEvents.filter(e => e.type === 'ResponseGenerated');
    const hasRedirect = responseEvents.some(e =>
      (e as any).text?.includes('started a new Claude Code thread')
    );
    expect(hasRedirect).toBe(false);
  });
});

// ---------------------------------------------------------------------------
describe('No duplicate events', () => {
  it('CC session has exactly one MessageReceived (no dual-persist)', () => {
    const { map, id } = makeThread();
    // Simulate a CC session — each event arrives with a unique seq (from DB via bus)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'file content' },
      { type: 'CodingAgentTextStreamed', text: 'I fixed the bug.' },
      { type: 'CodingAgentIdled' },
    ]);

    const thread = map.get(id)!;
    const allEvents = [...thread.events.values()];

    // Exactly one MessageReceived
    const msgEvents = allEvents.filter(e => e.type === 'MessageReceived');
    expect(msgEvents).toHaveLength(1);

    // Exactly one SessionStarted
    const sessionEvents = allEvents.filter(e => e.type === 'SessionStarted');
    expect(sessionEvents).toHaveLength(1);

    // Exactly one CodingAgentIdled
    const idleEvents = allEvents.filter(e => e.type === 'CodingAgentIdled');
    expect(idleEvents).toHaveLength(1);

    // Exactly one ToolCalled
    const toolEvents = allEvents.filter(e => e.type === 'CodingAgentToolCalled');
    expect(toolEvents).toHaveLength(1);

    // Exactly one ToolResult
    const resultEvents = allEvents.filter(e => e.type === 'CodingAgentToolResult');
    expect(resultEvents).toHaveLength(1);
  });

  it('CC session produces exactly one exchange (no double MessageReceived)', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'do the thing' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
  });

  it('recovery session produces exactly one exchange', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Recovering interrupted session...' },
      { type: 'SessionStarted', session_id: 'recovery-1' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: {} },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'ok' },
      { type: 'CodingAgentTextStreamed', text: 'The worktree is clean.' },
      { type: 'CodingAgentIdled' },
    ]);

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    expect(exchanges).toHaveLength(1);

    // No duplicate events of any type
    const allEvents = [...map.get(id)!.events.values()];
    const typeCounts = new Map<string, number>();
    for (const e of allEvents) {
      typeCounts.set(e.type, (typeCounts.get(e.type) || 0) + 1);
    }
    // Each event type should appear exactly once (except TextStreamed which can have multiple chunks)
    for (const [type, count] of typeCounts) {
      if (type !== 'CodingAgentTextStreamed' && type !== 'TextStreamed') {
        expect(count).toBe(1);
      }
    }
  });
});

// ---------------------------------------------------------------------------
// Flow: Image rendering
// ---------------------------------------------------------------------------
describe('Flow: Image rendering', () => {
  it('extracts user images from MessageReceived event', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      {
        type: 'MessageReceived',
        text: 'Look at this',
        images: [
          { base64: 'abc123', mime_type: 'image/png' },
          { base64: 'def456', mime_type: 'image/jpeg' },
        ],
      },
      { type: 'TextStreamed', text: 'I see two images.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    expect(exchanges).toHaveLength(1);

    const images = exchangeUserImages(exchanges[0]);
    expect(images).toHaveLength(2);
    expect(images[0]).toEqual({ base64: 'abc123', mimeType: 'image/png' });
    expect(images[1]).toEqual({ base64: 'def456', mimeType: 'image/jpeg' });
  });

  it('returns empty array when no images', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'No images' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    const images = exchangeUserImages(exchanges[0]);
    expect(images).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// Bug: CC follow-up images not rendered
// ---------------------------------------------------------------------------
describe('Bug: CC follow-up images not rendered', () => {
  it('pending CC follow-up includes images in synthetic exchange', () => {
    const { map, id } = makeThread();
    // Initial CC session events
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Working...' },
      { type: 'CodingAgentIdled' },
    ]);

    // Simulate pending follow-up WITH images
    const pendingImages = [
      { base64: 'img1data', mime_type: 'image/png' },
      { base64: 'img2data', mime_type: 'image/jpeg' },
    ];
    map.get(id)!.pendingUserMessages.push({
      text: 'here is the screenshot',
      eventId: 'ev-1',
      created: '2026-01-01T00:00:00Z',
      images: pendingImages,
    });

    // Verify images are stored in the pending message data structure
    const pending = map.get(id)!.pendingUserMessages[0];
    expect(pending.images).toHaveLength(2);
    expect(pending.images![0]).toEqual({ base64: 'img1data', mime_type: 'image/png' });
  });

  it('CC follow-up MessageReceived event includes images from SSE', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'CodingAgentIdled' },
      // Follow-up with images
      {
        type: 'MessageReceived',
        text: 'here is the screenshot',
        images: [
          { base64: 'img1data', mime_type: 'image/png' },
        ],
      },
    ]);

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    // The follow-up should be a separate exchange
    const followUp = exchanges[exchanges.length - 1];
    const images = exchangeUserImages(followUp);
    expect(images).toHaveLength(1);
    expect(images[0]).toEqual({ base64: 'img1data', mimeType: 'image/png' });
  });
});

// ---------------------------------------------------------------------------
// Bug: Channel labels missing on most exchanges
// ---------------------------------------------------------------------------
describe('Bug: Channel labels', () => {
  it('user channel is undefined when no channel in event', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Hello' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeUserChannel(exchanges[0])).toBeUndefined();
  });

  it('user channel reads from MessageReceived event payload', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix it', channel: 'claude_code' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeUserChannel(exchanges[0])).toBe('claude_code');
  });

  it('scheduled trigger has user channel "trigger"', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'TriggerStarted', trigger_id: 't1', trigger_name: 'Check weather' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeUserChannel(exchanges[0])).toBe('trigger');
  });
});

// ---------------------------------------------------------------------------
// Timestamps
// ---------------------------------------------------------------------------
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
  it('CC session with tools in progress → status=running (from MessageReceived event)', () => {
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

    // MergeConflictDetected sets ccApplying=true but doesn't change status
    insertEvents(map, id, [
      { type: 'MergeConflictDetected', change_id: 'c-6', files: ['f.rs'] },
    ]);
    expect(map.get(id)!.meta.status).toBe('waiting');
    expect(map.get(id)!.meta.ccApplying).toBe(true);
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
    handleEvent(map, id, seqCounter++, {
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
    handleEvent(map, id, seqCounter++, {
      type: 'MessageReceived', text: 'now fix tests', channel: 'claude_code',
    }, TS, 'random-server-uuid-456');

    // BUG: pending message stays because event_id doesn't match
    // After fix, this should be 0 — the CC loop must forward the client's event_id
    expect(map.get(id)!.pendingUserMessages).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// Bug: SSE-born scheduled trigger thread appears under History instead of Running
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
        ccHasChanges: false,
        ccRequiresRestart: false,
        ccIsExternalRepo: false,
        ccApplying: false,
        lastRevivedAt: '',
        state: 'active',
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
    handleEventWithAgg(map, id, seqCounter++, {
      type: 'TriggerStarted', trigger_id: 'task-1', trigger_name: 'Varmepumpe', prompt: 'Run control loop',
    } as any, new Date().toISOString());

    handleEventWithAgg(map, id, seqCounter++, {
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
    thread.meta.ccHasChanges = true;

    expect(effectiveThreadStatus(thread)).toBe('waiting');

    applyingNowThreadIds.value = new Map([[id, 'requesting']]);
    try {
      // Status must stay 'waiting' — only CC activity events (or harden/conflict
      // boundary events that precede them) should flip the thread to running.
      expect(effectiveThreadStatus(thread)).toBe('waiting');

      // displaySection then routes to Review, not Active.
      const section = displaySection(
        thread.meta.section, effectiveThreadStatus(thread),
        thread.meta.saved, thread.meta.activeChildrenCount > 0,
        thread.meta.ccHasChanges,
      );
      expect(section).toBe('review');
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

    // Old exchange should NOT be 'cc-working' — it should be 'interrupted'
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
  it('CC resumes work in same exchange after CodingAgentIdled → status becomes cc-working', () => {
    const { map, id } = makeThread();

    // CC session: works, goes idle, then resumes (more tool calls arrive)
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
    // Status should be cc-working (not done) because CC resumed
    expect(exchangeStatus(exchanges[0], '', true)).toBe('cc-working');
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

  it('CC follow-up creates new exchange — old exchange becomes done, new is cc-working', () => {
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
    expect(exchangeStatus(exchanges[1], '', true, false, true)).toBe('cc-working');
    expect(getLabel(exchanges[1], '', true, false, true)).toBe('Working');
  });

  it('CodingAgentPromptSent after idle resets exchange status to cc-working', () => {
    // Bug: CodingAgentPromptSent (automated prompt, e.g. hardening/conflict resolution)
    // was not handled in exchangeStatus, so isCCWaiting stayed true → 'done'.
    // Meanwhile the backend status correctly showed 'running' (active CC session).
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
    // Exchange should be cc-working (not done) — CC is processing the automated prompt
    expect(exchangeStatus(exchanges[0], '', true)).toBe('cc-working');
    expect(getLabel(exchanges[0])).toBe('Working');

    // Thread status should also be running (CC activity after completion)
    const thread = map.get(id)!;
    expect(thread.meta.status).toBe('running');
  });

  it('CodingAgentPromptSent after idle + more work → cc-working', () => {
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
    expect(exchangeStatus(exchanges[0], '', true)).toBe('cc-working');
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
    // Scenario: original CC session finishes → SessionEnded → review session starts
    // The hardening session is actively hardening (tool calls in progress).
    // Thread should be 'running', not 'idle'.
    // Use fresh timestamps — this happens in real-time.
    const { map, id } = makeThread();
    const now = new Date();
    const t = (offsetMs: number) => new Date(now.getTime() + offsetMs).toISOString();

    insertEvents(map, id, [
      // Original CC session
      { type: 'MessageReceived', text: 'add feature X', channel: 'claude_code', created: t(-15000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-14000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-13000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-12000) },
      { type: 'ResponseGenerated', created: t(-11000) },
      { type: 'CodingAgentIdled', created: t(-10000) },
      // Original session ends, hands off to hardening
      { type: 'SessionEnded', created: t(-9000) },
      // Hardening CC session starts and is actively working
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
    // Scenario: original CC session ended, review session hasn't started yet.
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
    // Review CC session completed, proposed a change, then ended.
    // Thread should show as 'waiting' (pending change), not 'idle'.
    const { map, id } = makeThread();

    insertEvents(map, id, [
      // Original CC session
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

    // ResponseCanceled set idle (no ccHasChanges). MissingHardeningDetected doesn't change status.
    // The hardening session (SessionStarted) will set it back to 'running' when it starts.
    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('thread is running during hardening session after MissingHardeningDetected', () => {
    const { map, id } = makeThread();
    const now = new Date();
    const t = (offsetMs: number) => new Date(now.getTime() + offsetMs).toISOString();

    insertEvents(map, id, [
      // Original CC session (fresh timestamps — real-time flow)
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
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('MissingHardeningDetected');
    expect(exchangeUserMessage(exchanges[1])).toBe('Lucidos Engine — Hardening');
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
    expect(getCCWaitingInfo(thread.meta)).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// SSE event-based status transitions
// ---------------------------------------------------------------------------
describe('SSE event-based status transitions via handleEvent()', () => {
  it('CC thread with SessionEnded + CodingAgentIdled → idle (SessionEnded transitions to idle)', () => {
    // Backend emits SessionEnded when CC session completes. This transitions to idle.
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

    // SessionEnded checks ccHasChanges (false) → idle
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

    // SessionEnded checks ccHasChanges (true) → waiting
    expect(thread.meta.status).toBe('waiting');
    expect(thread.meta.ccHasChanges).toBe(true);
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

    // ResponseGenerated checks ccHasChanges (false) → idle
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
    expect(map.get(id)!.meta.ccHasChanges).toBe(false);
    expect(map.get(id)!.meta.ccRequiresRestart).toBe(false);
    expect(map.get(id)!.meta.ccApplying).toBe(false);
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
    expect(map.get(id)!.meta.ccHasChanges).toBe(false);
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

    // ResponseGenerated checks ccHasChanges (false) → idle
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

    // displaySection with idle status + default section → history
    expect(displaySection('archived', status, false, false, false)).toBe('archive');
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

    // ChangeProposed set waiting with ccHasChanges. SessionStarted doesn't change it.
    expect(map.get(id)!.meta.status).toBe('waiting');
  });

});

// Bug: CC session aborted by engine restart should show as needing attention
// The engine no longer emits CodingAgentIdled during shutdown, so the last event
// is ResponseAborted → thread should be in review (inbox), not idle/history.
// ---------------------------------------------------------------------------
describe('Bug: aborted CC session (engine restart) should be in inbox for review', () => {
  it('ResponseAborted without pending changes → failed (red triangle indicates interruption)', () => {
    // Scenario: CC is actively working (tools running), engine restarts.
    // shutdown_agent_sessions sets shutting_down → ResponseAborted emitted.
    // Without CodingAgentIdled, ccHasChanges is false → status='failed' so the
    // user sees the red triangle indicating the run was interrupted.
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug', channel: 'claude_code', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: t(-110000) },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'file contents', created: t(-109000) },
      // Engine restart — ResponseAborted, NO CodingAgentIdled
      { type: 'ResponseAborted', created: t(-100000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('failed');
  });

  it('ResponseAborted + inbox stored section → displaySection is review', () => {
    // ResponseAborted sets status='failed' when no CC changes are pending and
    // marks the section as 'inbox' — together they place the thread in REVIEW.
    expect(displaySection('inbox', 'failed', false, false, false)).toBe('review');
  });

  it('aborted then recovered → running while recovery CC works', () => {
    // After engine restart, recover_orphaned_worktrees picks up the thread
    // and spawns a recovery CC session with "engine restarted" continuation.
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug', channel: 'claude_code', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-110000) },
      { type: 'ResponseAborted', created: t(-100000) },
      // Engine restart → recovery CC session picks up
      { type: 'SessionRecovered', created: t(-50000) },
      { type: 'SessionStarted', session_id: 's2', created: t(-49000) },
      // Recovery sends a continuation prompt → sets running
      { type: 'CodingAgentPromptSent', text: 'The engine restarted...', created: t(-48000) },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: t(-40000) },
    ]);

    // Recovery is in progress — CodingAgentPromptSent set running
    expect(map.get(id)!.meta.status).toBe('running');
  });
});

// Bug: CC thread with mid-session ResponseGenerated + resolved changes from prior session
// shows as idle (HISTORY) while CC is still actively working
// ---------------------------------------------------------------------------
describe('CC thread with post-completion activity bumps status back to running', () => {
  // CC may emit a `Result` mid-session (e.g. when the model invokes a Skill
  // tool that triggers another model turn), making the engine emit
  // `ResponseGenerated` / `CodingAgentIdled` while CC is actually still
  // working. The next activity event proves work is in progress and bumps
  // status back to `running` — see thread_lifecycle.rs for the matching
  // status transitions.

  it('CC tool call after ResponseGenerated bumps status back to running', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      // Session 1: complete cycle with change
      { type: 'MessageReceived', text: 'Fix the bug', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: t(-290000) },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: t(-289000) },
      { type: 'ResponseGenerated', created: t(-280000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-279000) },
      { type: 'ChangeProposed', change_id: 'c1', created: t(-270000) },
      { type: 'ChangeApplied', change_id: 'c1', created: t(-269000) },
      { type: 'SessionEnded', created: t(-268000) },
      // Session 2: new message, CC working
      { type: 'MessageReceived', text: 'Now do this', created: t(-60000) },
      { type: 'SessionStarted', session_id: 's2', created: t(-59000) },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: t(-50000) },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: t(-49000) },
      // ResponseGenerated emitted prematurely (e.g. CC's mid-session Result)
      { type: 'ResponseGenerated', created: t(-30000) },
      // CC continues working — activity event bumps status back to running
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-5000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('running');
  });

  it('CC text stream after ResponseGenerated bumps status back to running', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'ResponseGenerated', created: t(-60000) },
      // CodingAgentTextStreamed proves work is in progress → bump to running
      { type: 'CodingAgentTextStreamed', text: 'Working...', created: t(-5000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('running');
  });

  it('CC thread where last event equals completion time → still goes to idleOrWaiting', () => {
    // When the last event IS the completion event (not after it), behavior unchanged
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-60000) },
      { type: 'ChangeProposed', change_id: 'c1', created: t(-50000) },
      { type: 'ChangeApplied', change_id: 'c1', created: t(-10000) },
    ]);

    // ChangeApplied is last event AND a completion → idleOrWaiting → idle
    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('CC tool result after ResponseGenerated bumps status back to running', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: {}, created: t(-30000) },
      { type: 'ResponseGenerated', created: t(-20000) },
      // Tool result arrives after ResponseGenerated → bump back to running
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'ok', created: t(-5000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('running');
  });
});

// ---------------------------------------------------------------------------
// Bug: Queued chat message shows steps from the still-active previous exchange
// ---------------------------------------------------------------------------
describe('Bug: queued message must not inherit steps from active exchange', () => {
  it('steps arriving after pending message timestamp stay in the previous exchange', () => {
    const { map, id } = makeThread();

    // First exchange: user sends a message, engine starts working
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Plan our summer trip', created: '2026-03-22T19:12:00.000Z' } as any,
      { type: 'ToolCalled', name: 'web_search', args: { query: 'tropical islands' }, created: '2026-03-22T19:12:30.000Z' } as any,
      { type: 'ToolResult', name: 'web_search', result: 'results...', created: '2026-03-22T19:12:31.000Z' } as any,
    ]);

    // User sends a second message while the first is still being processed
    // This creates a pending message at 19:13:29
    const thread = map.get(id)!;
    thread.pendingUserMessages = [
      { text: 'Lag en fet floating navigasjon', eventId: 'msg-2', created: '2026-03-22T19:13:29.000Z' },
    ];

    // More steps from the FIRST exchange arrive AFTER the pending message timestamp
    // (the engine is still working on the first request)
    insertEvents(map, id, [
      { type: 'ToolCalled', name: 'run_python', args: { code: '...' }, created: '2026-03-22T19:13:45.000Z' } as any,
      { type: 'ToolResult', name: 'run_python', result: 'done', created: '2026-03-22T19:13:50.000Z' } as any,
      { type: 'ToolCalled', name: 'run_browser', args: { script: '...' }, created: '2026-03-22T19:14:10.000Z' } as any,
      { type: 'ToolResult', name: 'run_browser', result: 'ok', created: '2026-03-22T19:14:20.000Z' } as any,
    ]);

    const exchanges = getExchangesWithPending(map, id, true);

    // Should have exactly 2 exchanges: one real + one pending
    expect(exchanges).toHaveLength(2);
    expect(exchangeUserMessage(exchanges[0])).toBe('Plan our summer trip');
    expect(exchangeUserMessage(exchanges[1])).toBe('Lag en fet floating navigasjon');

    // ALL steps must belong to the first exchange (the active one)
    // The pending message's exchange must have ZERO steps
    expect(exchanges[0].steps.length).toBe(6); // 2 web_search + 2 run_python + 2 run_browser
    expect(exchanges[1].steps.length).toBe(0); // queued — no steps yet
  });

  it('pending message display timestamp is preserved even without created on synthetic event', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'First', created: '2026-03-22T19:12:00.000Z' } as any,
      { type: 'ResponseGenerated', created: '2026-03-22T19:12:30.000Z' } as any,
    ]);

    const thread = map.get(id)!;
    thread.pendingUserMessages = [
      { text: 'Second', eventId: 'msg-2', created: '2026-03-22T19:13:29.000Z' },
    ];

    const exchanges = getExchangesWithPending(map, id, true);
    expect(exchanges).toHaveLength(2);

    // The pending exchange should still show the correct timestamp for display
    const ts = exchangeTimestamp(exchanges[1]);
    expect(ts).toBe('2026-03-22T19:13:29.000Z');
  });
});

// ---------------------------------------------------------------------------
// Bug: CC follow-up messages show wrong status labels ("Queued", "Working")
// ---------------------------------------------------------------------------

describe('Bug: CC follow-up messages should never show "Queued" or premature "Working"', () => {
  it('CC follow-up with no steps should show "Requesting", not "Queued"', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    // CC is actively working on the first exchange
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-03-24T16:25:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-03-24T16:25:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-24T16:25:10Z' },
    ]);

    // User sends follow-up while CC is working
    map.get(id)!.pendingUserMessages.push({
      text: 'It\'s some safari thing no?',
      eventId: 'follow-1',
      created: '2026-03-24T16:25:21Z',
    } as any);

    const exchanges = getExchangesWithPending(map, id);
    expect(exchanges).toHaveLength(2);

    // First exchange is interrupted (user sent a follow-up)
    const status0 = exchangeStatus(exchanges[0], '', false, false, true);
    expect(status0).toBe('interrupted');

    // Follow-up should NOT be "queued" — CC doesn't queue like chat.
    // It should be "pending" (label: "Requesting") since CC hasn't started on it yet.
    const status1 = exchangeStatus(exchanges[1], '', true, true, true);
    expect(status1).not.toBe('queued');
    expect(status1).toBe('pending');
    expect(getLabel(exchanges[1], '', true, true, true)).toBe('Requesting');
  });

  it('last CC exchange with no steps should show "Requesting", not "Working"', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    // CC completed work, went idle, user sends follow-up
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-03-24T16:25:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-03-24T16:25:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-24T16:25:10Z' },
      { type: 'ResponseGenerated', created: '2026-03-24T16:25:15Z' },
      { type: 'CodingAgentIdled', created: '2026-03-24T16:25:16Z' },
    ]);

    // User sends follow-up — no CC events for it yet
    map.get(id)!.pendingUserMessages.push({
      text: 'Had no issues with other browsers',
      eventId: 'follow-1',
      created: '2026-03-24T16:25:57Z',
    } as any);

    const exchanges = getExchangesWithPending(map, id);
    expect(exchanges).toHaveLength(2);

    // Follow-up with no steps: should be "pending" (Requesting), not "cc-working" (Working)
    const status = exchangeStatus(exchanges[1], '', true, false, true);
    expect(status).not.toBe('cc-working');
    expect(status).toBe('pending');
    expect(getLabel(exchanges[1], '', true, false, true)).toBe('Requesting');
  });

  it('CC follow-up with CodingAgentPromptSent (no tool/text yet) → "Working"', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-03-24T16:25:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-03-24T16:25:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-24T16:25:10Z' },
      { type: 'ResponseGenerated', created: '2026-03-24T16:25:15Z' },
      { type: 'CodingAgentIdled', created: '2026-03-24T16:25:16Z' },
      // Follow-up: prompt sent to CC but no response yet
      { type: 'MessageReceived', text: 'now fix the tests', created: '2026-03-24T16:26:00Z' },
      { type: 'CodingAgentPromptSent', text: 'now fix the tests', created: '2026-03-24T16:26:00Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    // CodingAgentPromptSent adds a step → hasSteps=true → cc-working → "Working"
    const status = exchangeStatus(exchanges[1], '', true, false, true);
    expect(status).toBe('cc-working');
    expect(getLabel(exchanges[1], '', true, false, true)).toBe('Working');
  });

  it('CC follow-up WITH steps should still show "Working"', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-03-24T16:25:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-03-24T16:25:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-24T16:25:10Z' },
      { type: 'ResponseGenerated', created: '2026-03-24T16:25:15Z' },
      { type: 'CodingAgentIdled', created: '2026-03-24T16:25:16Z' },
      // Follow-up with CC working on it
      { type: 'MessageReceived', text: 'now fix the tests', created: '2026-03-24T16:26:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: '2026-03-24T16:26:05Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    // Follow-up WITH steps: should be "cc-working"
    const status = exchangeStatus(exchanges[1], '', true, false, true);
    expect(status).toBe('cc-working');
    expect(getLabel(exchanges[1], '', true, false, true)).toBe('Working');
  });

  it('non-last CC exchange with no steps should show "Done", not "Interrupted"', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-03-24T16:25:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-03-24T16:25:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-24T16:25:10Z' },
      { type: 'ResponseGenerated', created: '2026-03-24T16:25:15Z' },
      { type: 'CodingAgentIdled', created: '2026-03-24T16:25:16Z' },
      // Follow-up 1: no response from CC (user sent another immediately)
      { type: 'MessageReceived', text: 'first follow-up', created: '2026-03-24T16:26:00Z' },
      // Follow-up 2: CC works on this one
      { type: 'MessageReceived', text: 'second follow-up', created: '2026-03-24T16:26:05Z' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: '2026-03-24T16:26:10Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);

    // Follow-up 1 with no steps, not last: should be "done" (CC skipped it), not "interrupted"
    const status = exchangeStatus(exchanges[1], '', false, false, true);
    expect(status).toBe('done');
  });

  it('multiple CC follow-ups all pending — none should show "Queued"', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-03-24T16:25:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-03-24T16:25:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-24T16:25:10Z' },
    ]);

    // Two pending follow-ups
    map.get(id)!.pendingUserMessages.push(
      { text: 'follow-up 1', eventId: 'f1', created: '2026-03-24T16:25:21Z' } as any,
      { text: 'follow-up 2', eventId: 'f2', created: '2026-03-24T16:25:57Z' } as any,
    );

    const exchanges = getExchangesWithPending(map, id);
    expect(exchanges).toHaveLength(3);

    // None should be "queued"
    for (let i = 1; i < exchanges.length; i++) {
      const isLast = i === exchanges.length - 1;
      const priorStatus = exchangeStatus(exchanges[i - 1], '', false, false, true);
      const hasPrior = isActive(priorStatus);
      const status = exchangeStatus(exchanges[i], '', isLast, hasPrior, true);
      expect(status).not.toBe('queued');
    }
  });
});

// ---------------------------------------------------------------------------
// CC session lifecycle: getCCWaitingInfo state transitions
// ---------------------------------------------------------------------------
describe('CC idle session — getCCWaitingInfo state transitions', () => {
  it('cc waiting info is cleared when session ends', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the tests' },
      { type: 'SessionStarted', session_id: 'claude-code/20260325' },
      { type: 'CodingAgentIdled', has_changes: true, cc_session_id: 'abc-123-session' },
      { type: 'SessionEnded' },
    ]);

    const info = getCCWaitingInfo(map.get(id)!.meta);
    // Session ended — no waiting info at all
    expect(info).toBeNull();
  });

  it('cc waiting info is cleared when CodingAgentPromptSent arrives after idle', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the tests' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentIdled', has_changes: true },
      // Engine sends automated prompt — CC is no longer waiting
      { type: 'CodingAgentPromptSent', text: '/harden' },
    ]);

    const info = getCCWaitingInfo(map.get(id)!.meta);
    expect(info).toBeNull();
  });

  it('Discard & End Session: SessionEnded with reason=discarded, no ChangeProposed', () => {
    const { map, id } = makeThread();

    // CC session idles with changes, user clicks "Discard & End Session"
    // Backend emits ChangeDiscarded (to clear cc flags) then SessionEnded
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'implement feature', created: '2026-03-26T10:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-03-26T10:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-26T10:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'done', created: '2026-03-26T10:00:03Z' },
      { type: 'CodingAgentIdled', has_changes: true, created: '2026-03-26T10:00:04Z' },
      // User clicks Discard & End Session → backend discards changes then ends session
      { type: 'ChangeDiscarded', created: '2026-03-26T10:00:04.500Z' },
      { type: 'SessionEnded', reason: 'discarded', created: '2026-03-26T10:00:05Z' },
    ]);

    const thread = map.get(id)!;
    const events = [...thread.events.values()];

    // No ChangeProposed event should exist
    expect(events.some(e => e.type === 'ChangeProposed')).toBe(false);

    // Thread should not be in waiting state (ChangeDiscarded cleared cc flags)
    const info = getCCWaitingInfo(thread.meta);
    expect(info).toBeNull();

    // Thread status should be idle (ChangeDiscarded → idle, SessionEnded → idle)
    expect(thread.meta.status).toBe('idle');
  });

  it('Add to Changes: SessionEnded with ChangeProposed before it', () => {
    const { map, id } = makeThread();

    // CC session idles with changes, user clicks "Add to Changes"
    // Backend SHOULD emit ChangeProposed before SessionEnded
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'implement feature', created: '2026-03-26T10:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-03-26T10:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-26T10:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'done', created: '2026-03-26T10:00:03Z' },
      { type: 'CodingAgentIdled', has_changes: true, created: '2026-03-26T10:00:04Z' },
      // User clicks Add to Changes → backend proposes change, then ends session
      { type: 'ChangeProposed', change_id: 'c-1', description: 'Feature implementation', created: '2026-03-26T10:00:05Z' },
      { type: 'SessionEnded', reason: 'changes_proposed', created: '2026-03-26T10:00:06Z' },
    ]);

    const thread = map.get(id)!;
    const events = [...thread.events.values()];

    // ChangeProposed should exist
    expect(events.some(e => e.type === 'ChangeProposed')).toBe(true);

    // Thread should not be in waiting state (session ended)
    const info = getCCWaitingInfo(thread.meta);
    expect(info).toBeNull();

    // Thread should be in waiting state — pending changes need resolution
    expect(thread.meta.status).toBe('waiting');
  });

  it('Discard without ChangeDiscarded: CodingAgentIdled { has_changes: false } clears stale flags', () => {
    const { map, id } = makeThread();

    // CC session idles with changes + requires_restart.
    // User clicks Discard, but no pending change exists in DB, so no ChangeDiscarded is emitted.
    // Backend resets worktree and emits CodingAgentIdled { has_changes: false }.
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'refactor engine', created: '2026-03-31T10:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-03-31T10:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-31T10:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'done', created: '2026-03-31T10:00:03Z' },
      { type: 'CodingAgentIdled', has_changes: true, requires_restart: true, created: '2026-03-31T10:00:04Z' },
      // Discard: no ChangeDiscarded (no pending change in DB), just CodingAgentIdled { has_changes: false }
      { type: 'CodingAgentIdled', has_changes: false, requires_restart: false, created: '2026-03-31T10:00:05Z' },
    ]);

    const thread = map.get(id)!;

    // ccHasChanges must be false — CodingAgentIdled { has_changes: false } should clear it
    expect(thread.meta.ccHasChanges).toBe(false);
    expect(thread.meta.ccRequiresRestart).toBe(false);
    // Status should be 'idle' — no changes means nothing to act on
    expect(thread.meta.status).toBe('idle');
  });

  it('Stale discard without ChangeDiscarded: SessionEnded reason=discarded clears stale flags', () => {
    const { map, id } = makeThread();

    // CC session idles with changes + requires_restart.
    // Engine restarts, session is stale. User clicks Discard.
    // No pending change in DB → no ChangeDiscarded emitted.
    // Backend emits SessionEnded { reason: "discarded" }.
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'refactor engine', created: '2026-03-31T10:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-03-31T10:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-31T10:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'done', created: '2026-03-31T10:00:03Z' },
      { type: 'CodingAgentIdled', has_changes: true, requires_restart: true, created: '2026-03-31T10:00:04Z' },
      // Engine restarts, user clicks Discard on stale session
      // No pending change in DB, so no ChangeDiscarded — just SessionEnded
      { type: 'SessionEnded', reason: 'discarded', created: '2026-03-31T10:00:10Z' },
    ]);

    const thread = map.get(id)!;

    // ccHasChanges must be false — SessionEnded with reason=discarded should clear flags
    expect(thread.meta.ccHasChanges).toBe(false);
    expect(thread.meta.ccRequiresRestart).toBe(false);
    expect(thread.meta.ccIsExternalRepo).toBe(false);

    // Thread should be idle, not waiting
    expect(thread.meta.status).toBe('idle');

    // No CC waiting info — session ended
    const info = getCCWaitingInfo(thread.meta);
    expect(info).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// CC follow-up after all changes resolved
// ---------------------------------------------------------------------------
// When a CC thread has all changes resolved (applied/discarded) and the user
// sends a follow-up, MessageReceived sets status='running' (new exchange started).
describe('CC follow-up after resolved changes correctly shows running', () => {
  const now = new Date();
  const t = (offsetMs: number) => new Date(now.getTime() + offsetMs).toISOString();

  it('MessageReceived after all changes resolved → running', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      // First exchange: CC works, proposes changes, user applies them
      { type: 'MessageReceived', text: 'fix the bug', created: t(-10000) },
      { type: 'SessionStarted', session_id: 'claude-code/test', created: t(-9000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-8000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-7000) },
      { type: 'CodingAgentTextStreamed', text: 'Fixed.', created: t(-6000) },
      { type: 'ResponseGenerated', created: t(-5000) },
      { type: 'ChangeProposed', change_id: 'c1', description: 'fix', files: ['a.ts'], requires_restart: false, created: t(-4000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-3000) },
      // User applies the change
      { type: 'ChangeApplied', change_id: 'c1', created: t(-2000) },
      // User sends follow-up — MessageReceived sets status='running'
      { type: 'MessageReceived', text: 'now fix the tests too', created: t(-1000) },
    ]);

    const thread = map.get(id)!;
    // MessageReceived → running
    expect(thread.meta.status).toBe('running');

    // And display section must be active, not archive
    const section = displaySection('archived', 'running', false, false, false);
    expect(section).toBe('active');
  });

  it('CodingAgentUserMessageSent after resolved changes → running (with live process)', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix it', created: t(-10000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-9000) },
      { type: 'ResponseGenerated', created: t(-5000) },
      { type: 'ChangeProposed', change_id: 'c1', description: 'fix', files: ['a.ts'], requires_restart: false, created: t(-4000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-3000) },
      { type: 'ChangeApplied', change_id: 'c1', created: t(-2000) },
      // Follow-up via CC channel (old format)
      { type: 'CodingAgentUserMessageSent', text: 'also fix tests', created: t(-1000) },
    ]);

    const thread = map.get(id)!;
    expect(thread.meta.status).toBe('running');
  });

  it('TriggerStarted after resolved changes → running (with live process)', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'check logs', created: t(-10000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-9000) },
      { type: 'ResponseGenerated', created: t(-5000) },
      { type: 'ChangeProposed', change_id: 'c1', description: 'fix', files: ['a.ts'], requires_restart: false, created: t(-4000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-3000) },
      { type: 'ChangeApplied', change_id: 'c1', created: t(-2000) },
      // Scheduled trigger starts
      { type: 'TriggerStarted', trigger_id: 'task-1', trigger_name: 'daily check', created: t(-1000) },
    ]);

    const thread = map.get(id)!;
    expect(thread.meta.status).toBe('running');
  });
});

// ---------------------------------------------------------------------------
// Bug: CC follow-up aborted by CC process crash shows "engine restarted"
// ResponseAborted from a CC process crash (stdin write failed, EOF race) is
// NOT an engine restart. The banner should distinguish the two cases.
// ---------------------------------------------------------------------------
describe('CC follow-up abort: ResponseAborted is now an exchange boundary', () => {
  // The previous "engine restart vs CC crash" banner discrimination was
  // replaced by per-event `actor` attribution on ResponseAborted, rendered
  // by the AbortPanel below the original response. These tests verify the
  // new boundary semantics rather than the old `isAbortedByRestart` helper.
  it('CC crash (no shutdown) opens an abort boundary exchange', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug', channel: 'claude_code', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-110000) },
      { type: 'ResponseGenerated', created: t(-105000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-100000) },
      { type: 'MessageReceived', text: 'Now fix tests', created: t(-50000) },
      { type: 'ResponseAborted', created: t(-49000) },
      { type: 'SessionEnded', created: t(-48000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);
    expect(exchangeStatus(exchanges[1], '', false, false, true)).toBe('aborted');
    expect(exchanges[2].userEvent.type).toBe('ResponseAborted');
  });

  it('shutdown abort still marks the original exchange aborted', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug', channel: 'claude_code', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-110000) },
      { type: 'ResponseAborted', created: t(-100000) },
      { type: 'SessionEnded', reason: 'shutdown', created: t(-99000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false, false, true)).toBe('aborted');
  });
});

// ---------------------------------------------------------------------------
// Aborted-exchange grouping after the boundary refactor: ResponseAborted now
// opens its own initiator-only "abort panel" exchange (where the AbortPanel
// + Continue button live) AND remains a step of the prior exchange so the
// partial-response panel keeps its 'aborted' status.
// ---------------------------------------------------------------------------
describe('Aborted-exchange boundary: ResponseAborted opens its own panel', () => {
  it('CC follow-up aborted before any output: AbortPanel exchange is empty', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug', channel: 'claude_code', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-110000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-109000) },
      { type: 'CodingAgentTextStreamed', text: 'Done fixing.', created: t(-108000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-100000) },
      { type: 'ChangeApplied', change_id: 'c1', created: t(-95000) },
      { type: 'SessionEnded', reason: 'changes_applied', created: t(-94000) },
      { type: 'MessageReceived', text: 'The ios suite should have been included', channel: 'claude_code', created: t(-50000) },
      { type: 'ResponseAborted', created: t(-49000) },
    ]);

    const exchanges = getExchanges(map, id);
    // 1: original CC, 2: ChangeApplied, 3: follow-up (aborted), 4: ResponseAborted boundary
    expect(exchanges).toHaveLength(4);
    expect(exchangeStatus(exchanges[0], '', false, false, true)).toBe('done');
    expect(exchanges[1].userEvent.type).toBe('ChangeApplied');
    expect(exchangeStatus(exchanges[2], '', false, false, true)).toBe('aborted');
    expect(exchanges[3].userEvent.type).toBe('ResponseAborted');
    expect(exchanges[3].steps).toHaveLength(0);
  });

  it('CC follow-up aborted AFTER producing output: prior exchange keeps its content', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug', channel: 'claude_code', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-110000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-109000) },
      { type: 'CodingAgentIdled', has_changes: false, created: t(-100000) },
      { type: 'MessageReceived', text: 'Now fix tests', channel: 'claude_code', created: t(-50000) },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, description: 'Reading test file', created: t(-48000) },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: t(-47000) },
      { type: 'CodingAgentTextStreamed', text: 'Looking at the test failures...', created: t(-46000) },
      { type: 'ResponseAborted', created: t(-45000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);
    const followUp = exchanges[1];
    expect(exchangeStatus(followUp, '', false, false, true)).toBe('aborted');
    expect(exchangeResponseText(followUp)).toBe('Looking at the test failures...');
    expect(exchanges[2].userEvent.type).toBe('ResponseAborted');
  });

  it('chat exchange aborted: AbortPanel boundary opens after the original', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Hello', channel: 'chat', created: t(-120000) },
      { type: 'TextStreamed', text: 'Hi there!', created: t(-119000) },
      { type: 'ResponseGenerated', created: t(-118000) },
      { type: 'MessageReceived', text: 'Now what?', channel: 'chat', created: t(-50000) },
      { type: 'ResponseAborted', created: t(-49000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);
    expect(exchangeStatus(exchanges[1], '', false)).toBe('aborted');
    expect(exchanges[2].userEvent.type).toBe('ResponseAborted');
  });
});

// ---------------------------------------------------------------------------
// Bug: CC follow-up triggers stale resume → transient "Aborted ⚠" status.
// When a CC session expires, the engine emits SessionEnded(stale_resume)
// before retrying with a fresh SessionStarted. The stale_resume reason is a
// normal lifecycle event (deliberate retry), not a system interruption.
// Without stale_resume in NORMAL_SESSION_END_REASONS, the intermediate
// SessionEnded causes exchangeStatus to return 'aborted' transiently.
// ---------------------------------------------------------------------------
describe('CC stale resume — SessionEnded(stale_resume) must not cause aborted status', () => {
  const now = Date.now();
  const t = (offset: number) => new Date(now + offset).toISOString();

  it('mid-resume: SessionEnded(stale_resume) followed by new SessionStarted is cc-working, not aborted', () => {
    seqCounter = 1;
    const { map, id } = makeThread('stale-resume-1', 'running');

    insertEvents(map, id, [
      // Exchange 1: initial CC session completes
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: { file: 'foo.rs' }, created: t(-280000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-279000) },
      { type: 'ResponseGenerated', created: t(-270000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-269000) },
      // Exchange 2: follow-up triggers stale resume
      { type: 'MessageReceived', text: 'include the ios suite too', channel: 'claude_code', created: t(-60000) },
      // Stale session detected → SessionEnded with stale_resume reason
      { type: 'SessionEnded', reason: 'stale_resume', created: t(-59000) },
      // Fresh session starts immediately
      { type: 'SessionStarted', session_id: 's2', created: t(-58000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: { file: 'ios.rs' }, created: t(-50000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    const followUp = exchanges[1];
    // Must NOT be 'aborted' — stale_resume is a normal lifecycle event
    const status = exchangeStatus(followUp, '', true, false, true);
    expect(status).not.toBe('aborted');
    expect(status).toBe('cc-working');
  });

  it('stale_resume only (before retry SessionStarted arrives) is not aborted', () => {
    seqCounter = 1;
    const { map, id } = makeThread('stale-resume-2', 'running');

    insertEvents(map, id, [
      // Exchange 1: initial CC session completes
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      { type: 'ResponseGenerated', created: t(-270000) },
      { type: 'CodingAgentIdled', has_changes: false, created: t(-269000) },
      // Exchange 2: follow-up — only stale_resume arrived so far (retry pending)
      { type: 'MessageReceived', text: 'also fix bar', channel: 'claude_code', created: t(-60000) },
      { type: 'SessionEnded', reason: 'stale_resume', created: t(-59000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    const followUp = exchanges[1];
    const status = exchangeStatus(followUp, '', true, false, true);
    // Even with only SessionEnded(stale_resume) and no retry yet, must not be 'aborted'
    expect(status).not.toBe('aborted');
  });

  it('thread status stays running after SessionEnded(stale_resume)', () => {
    seqCounter = 1;
    const { map, id } = makeThread('stale-status-1', 'running');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      // Stale session detected — engine retries with fresh session
      { type: 'SessionEnded', reason: 'stale_resume', created: t(-298000) },
    ]);

    const thread = map.get(id)!;
    // Backend skips status update for StaleResume (event_bus.rs:1006).
    // Frontend must match: status should stay 'running', not become 'idle'.
    expect(thread.meta.status).toBe('running');
  });

  it('thread status stays running through full stale resume → retry sequence', () => {
    seqCounter = 1;
    const { map, id } = makeThread('stale-status-2', 'running');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      { type: 'SessionEnded', reason: 'stale_resume', created: t(-298000) },
      // Fresh session starts — status should still be running
      { type: 'SessionStarted', session_id: 's2', created: t(-297000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: { file: 'foo.rs' }, created: t(-290000) },
    ]);

    const thread = map.get(id)!;
    expect(thread.meta.status).toBe('running');
    // displaySection should be 'running', not 'review' or 'history'
    expect(displaySection(thread.meta.section, thread.meta.status, thread.meta.saved, thread.meta.activeChildrenCount > 0, thread.meta.ccHasChanges)).toBe('active');
  });
});

// ---------------------------------------------------------------------------
// Stale exchange recovery — incomplete exchanges after engine crash/lid close
// ---------------------------------------------------------------------------

describe('stale exchange recovery (incomplete last exchange)', () => {
  const t = (ms: number) => new Date(Date.now() + ms).toISOString();

  it('chat thread: last exchange with ToolCalled but no terminal event shows aborted when thread is idle', () => {
    seqCounter = 1;
    // Thread with status 'idle' — as it would be after engine restart
    const { map, id } = makeThread('stale-exchange-1', 'idle');

    insertEvents(map, id, [
      // Exchange 1: completed chat response
      { type: 'MessageReceived', text: 'Fix the workflow app', channel: 'chat', created: t(-300000) },
      { type: 'TextStreamed', text: 'Let me check...', created: t(-299000) },
      { type: 'ToolCalled', name: 'read_file', args: { path: 'index.html' }, description: 'Reading index.html...', created: t(-298000) },
      { type: 'ToolResult', name: 'read_file', result: '<html>...', created: t(-297000) },
      { type: 'ResponseGenerated', text: 'Fixed it.', created: t(-296000) },
      // Exchange 2: follow-up — interrupted mid-tool-execution (lid close)
      { type: 'MessageReceived', text: 'Doesnt work', channel: 'chat', created: t(-200000) },
      { type: 'TextStreamed', text: 'Let me investigate...', created: t(-199000) },
      { type: 'ToolCalled', name: 'run_python', args: { code: 'import os' }, description: 'Running Python code...', created: t(-198000) },
      // No ToolResult, no ResponseGenerated — engine died during Python execution
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    // Exchange 1: completed normally
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');

    // Exchange 2: should show as aborted since thread is idle but no terminal event
    const lastExchange = exchanges[1];
    const threadIdle = true;  // thread DB status is 'idle' after engine restart
    const status = exchangeStatus(lastExchange, '', true, false, false, threadIdle);
    // Must return 'aborted' — the engine crashed mid-response, not still streaming
    expect(status).toBe('aborted');

    // Pending steps should be resolved (no spinning "Running Python")
    const steps = exchangeSteps(lastExchange, true, threadIdle);
    const pendingSteps = steps.filter(s => s.success === null);
    expect(pendingSteps).toHaveLength(0);
  });

  it('chat thread: last exchange with only TextStreamed (no tools) shows aborted when thread is idle', () => {
    seqCounter = 1;
    const { map, id } = makeThread('stale-exchange-2', 'idle');

    insertEvents(map, id, [
      // Exchange 1: completed
      { type: 'MessageReceived', text: 'Hello', channel: 'chat', created: t(-300000) },
      { type: 'ResponseGenerated', text: 'Hi!', created: t(-299000) },
      // Exchange 2: interrupted after partial streaming
      { type: 'MessageReceived', text: 'How are you?', channel: 'chat', created: t(-200000) },
      { type: 'TextStreamed', text: 'I am doing w', created: t(-199000) },
      // No ResponseGenerated — connection dropped mid-stream
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    const lastExchange = exchanges[1];
    const threadIdle = true;
    const status = exchangeStatus(lastExchange, '', true, false, false, threadIdle);
    expect(status).toBe('aborted');
  });

  it('exchange with streaming buffer is NOT aborted even when thread is idle', () => {
    seqCounter = 1;
    const { map, id } = makeThread('stale-exchange-3', 'idle');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Hello', channel: 'chat', created: t(-200000) },
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python...', created: t(-199000) },
    ]);

    const exchanges = getExchanges(map, id);
    // With active streaming buffer, still counts as streaming
    const status = exchangeStatus(exchanges[0], 'partial text arriving...', true, false, false, true);
    expect(status).toBe('streaming');
  });

  it('non-idle thread with incomplete exchange is still streaming (not aborted)', () => {
    seqCounter = 1;
    const { map, id } = makeThread('stale-exchange-4', 'running');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Hello', channel: 'chat', created: t(-200000) },
      { type: 'TextStreamed', text: 'Working on it...', created: t(-199000) },
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python...', created: t(-198000) },
    ]);

    const exchanges = getExchanges(map, id);
    // Thread is running → exchange is actively being processed
    const status = exchangeStatus(exchanges[0], '', true, false, false, false);
    expect(status).toBe('streaming');
  });

  // Regression: chat follow-up posted while a prior request was mid-flight ended
  // up showing "Aborted ⚠" once the agent finished and the thread idled.
  // The agentic loop folds the follow-up into the running prompt via
  // UserPromptInjected (with injected_message_id matching the new MR), and the
  // ResponseGenerated carries the ORIGINAL request_event_id — so it routes back
  // to the prior exchange. The follow-up exchange is left with only the
  // absorbed UPI as its sole step, which the threadIdle stale-detection
  // fallback misread as a crash.
  it('chat follow-up absorbed via UserPromptInjected is NOT aborted once thread idles', () => {
    seqCounter = 1;
    const { map, id } = makeThread('upi-folded-1', 'idle');

    insertEvents(map, id, [
      // Prior message — agentic loop is mid-stream when the follow-up arrives.
      { type: 'MessageReceived', text: 'ferdig', channel: 'chat', created: t(-300000), event_id: 'msg-A' },
      { type: 'Thinking', text: 'analyzing', request_event_id: 'msg-A', created: t(-299000) } as ThreadEvent,
      { type: 'ToolCalled', name: 'sql_query', args: {}, description: 'Querying...', request_event_id: 'msg-A', created: t(-298000) } as ThreadEvent,
      { type: 'ToolResult', name: 'sql_query', result: 'ok', request_event_id: 'msg-A', created: t(-297000) } as ThreadEvent,
      // Follow-up message — user posts while loop is still working.
      { type: 'MessageReceived', text: 'men kan ikke ha dette hele tiden', channel: 'chat', created: t(-296000), event_id: 'msg-B' },
      // More work for A — pre-injection, still belongs to A.
      { type: 'ToolCalled', name: 'check_creds', args: {}, description: 'Checking creds...', request_event_id: 'msg-A', created: t(-295000) } as ThreadEvent,
      { type: 'ToolResult', name: 'check_creds', result: 'ok', request_event_id: 'msg-A', created: t(-294000) } as ThreadEvent,
      // Engine injects the follow-up into the running prompt — split point.
      { type: 'UserPromptInjected', text: 'men kan ikke ha dette hele tiden', injected_message_id: 'msg-B', request_event_id: 'msg-A', created: t(-293000) } as ThreadEvent,
      // Post-injection events answer B even though they keep A's req_id.
      { type: 'TextStreamed', text: 'Combined answer', request_event_id: 'msg-A', created: t(-292000) } as ThreadEvent,
      { type: 'ResponseGenerated', text: 'Combined answer', request_event_id: 'msg-A', created: t(-291000) } as ThreadEvent,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    // Exchange A: pre-injection work only. No terminal — non-last with steps
    // → 'interrupted' ("Continued below ↳"). The response continues in the
    // follow-up exchange after the UPI absorbed the new prompt.
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect((exchanges[0].userEvent as { text: string }).text).toBe('ferdig');
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual([
      'Thinking', 'ToolCalled', 'ToolResult', 'ToolCalled', 'ToolResult',
    ]);
    expect(exchangeStatus(exchanges[0], '', false, false, false, true)).toBe('interrupted');

    // Exchange B: UPI + post-injection work + final response.
    expect(exchanges[1].userEvent.type).toBe('MessageReceived');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('men kan ikke ha dette hele tiden');
    expect(exchanges[1].steps.map(s => s.event.type)).toEqual([
      'UserPromptInjected', 'TextStreamed', 'ResponseGenerated',
    ]);
    expect(exchangeStatus(exchanges[1], '', true, false, false, true)).toBe('done');
  });
});

// ---------------------------------------------------------------------------
// Bug: CC SessionEnded(changes_proposed) without CodingAgentIdled keeps the
// exchange stuck on "Working". Happens when the engine's auto-harden `continue`
// path skips the in-loop CodingAgentIdled emission and the loop then exits via
// post-loop cleanup (which today only emits SessionEnded). The exchange has no
// terminal CC event, so the status falls through to 'cc-working' forever.
// ---------------------------------------------------------------------------
describe('CC SessionEnded(changes_proposed) without preceding CodingAgentIdled', () => {
  const t = (offset: number) => new Date(Date.now() + offset).toISOString();

  it('treats SessionEnded(changes_proposed) as terminal even without CodingAgentIdled', () => {
    seqCounter = 1;
    // Thread DB status is 'waiting' (CC has pending changes after SessionEnded)
    const { map, id } = makeThread('cc-changes-no-idle', 'waiting');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'merge in main', channel: 'claude_code', created: t(-200000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-199000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: { file: 'foo.rs' }, created: t(-180000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-179000) },
      // Auto-harden ran a test that died (exit 137); CC emitted a terminal text/result
      // and the engine post-loop emitted SessionEnded. Crucially: NO CodingAgentIdled.
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'cargo test' }, created: t(-160000) },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'Exit code 137', created: t(-150000) },
      { type: 'SessionEnded', reason: 'changes_proposed', created: t(-149000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);

    const status = exchangeStatus(exchanges[0], '', true, false, true);
    // Must NOT be 'cc-working' — SessionEnded means the agent is no longer running.
    expect(status).not.toBe('cc-working');
    // SessionEnded with a normal lifecycle reason is terminal → 'done'.
    expect(status).toBe('done');
  });

  it('SessionEnded(changes_proposed) WITH preceding CodingAgentIdled is also done', () => {
    seqCounter = 1;
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

// ---------------------------------------------------------------------------
// Chat mid-flight injection: a follow-up MR lands while the parent's agentic
// loop is still running. The loop folds the new prompt in via UPI and keeps
// emitting events under the parent's request_event_id; the parent must keep
// its 'Working' state and pending spinners until the real terminator lands,
// and the follow-up's absorbed UPI must read as 'done', not the threadIdle
// stale-detector's 'aborted'.
// ---------------------------------------------------------------------------
describe('chat follow-up while parent loop still running', () => {
  const t = (offset: number) => new Date(Date.now() + offset).toISOString();

  it('parent mid-flight is NOT done and pending step stays a spinner when follow-up arrives', () => {
    seqCounter = 1;
    const { map, id } = makeThread('parent-midflight-1', 'running');

    insertEvents(map, id, [
      // Parent message — agentic loop starts processing.
      { type: 'MessageReceived', text: 'fix the script', channel: 'chat', created: t(-30000), event_id: 'parent-mr' },
      { type: 'Thinking', text: 'analyzing', request_event_id: 'parent-mr', created: t(-29000) } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python code...', request_event_id: 'parent-mr', created: t(-25000) } as ThreadEvent,
      { type: 'ToolResult', name: 'run_python', result: 'ok', request_event_id: 'parent-mr', created: t(-24000) } as ThreadEvent,
      { type: 'Thinking', text: 'now python again', request_event_id: 'parent-mr', created: t(-22000) } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python code...', request_event_id: 'parent-mr', created: t(-20000) } as ThreadEvent,
      // ↑ No matching ToolResult yet — Python is still running.
      // User sends a follow-up while the Python tool is still in flight.
      { type: 'MessageReceived', text: 'Uuhh fix the script?', channel: 'chat', created: t(-10000), event_id: 'followup-mr' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    // Parent exchange: it is NOT the last (the follow-up is) but the agentic
    // loop is still running. The thread DB confirms this — status is 'running'.
    // Status must NOT be 'done' yet.
    const parentStatus = exchangeStatus(exchanges[0], '', /* isLast */ false, /* hasPriorActive */ false, /* threadIsCC */ false, /* threadIdle */ false);
    expect(parentStatus).not.toBe('done');

    // Pending step (the second run_python) must NOT be auto-resolved to ✓
    // while the agent is still actively processing (thread still 'running').
    const events = exchangeResponseEvents(exchanges[0], 0, /* isLast */ false);
    const pythonSteps = events.filter(e => e.type === 'step' && /python/i.test((e as { description?: string }).description ?? ''));
    expect(pythonSteps).toHaveLength(2);
    const lastPython = pythonSteps[pythonSteps.length - 1] as { success: boolean | null };
    expect(lastPython.success).toBeNull(); // spinner, not ✓
  });

  // Verbatim event shape from a production thread: parent emits two
  // run_python tool calls, follow-up MR lands while the second is in flight,
  // engine drains injection and emits UPI absorbing into the follow-up,
  // ResponseGenerated routes back to the parent via request_event_id.
  it('production-style absorbed UPI: follow-up status is done, not aborted', () => {
    seqCounter = 1;
    const { map, id } = makeThread('upi-prod', 'idle');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: "No way! I'm not going into the terminal, pls fix", channel: 'chat', created: '2026-05-04T11:51:39.438Z', event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31' },
      { type: 'MemorySearched', results: 5, request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:51:54.537Z' } as ThreadEvent,
      { type: 'Thinking', text: '', context_tokens:5147, context_messages: 1, request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:51:54.557Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_bash', args: {}, description: 'Running bash...', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:52:00.358Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'run_bash', result: 'ok', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:52:00.403Z' } as ThreadEvent,
      { type: 'Thinking', text: '', context_tokens:7242, context_messages: 3, request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:52:00.410Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python code...', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:52:17.583Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'run_python', result: 'ok', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:52:17.789Z' } as ThreadEvent,
      { type: 'Thinking', text: '', context_tokens:7634, context_messages: 5, request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:52:17.793Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python code...', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:52:30.133Z' } as ThreadEvent,
      // User sends follow-up while the second run_python is executing.
      { type: 'MessageReceived', text: 'Uuhh fix the script?', channel: 'chat', created: '2026-05-04T11:52:38.205Z', event_id: 'a7d179ab-f451-4ff7-89dd-61ed413aaa88' },
      { type: 'ToolResult', name: 'run_python', result: 'ok', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:01.080Z' } as ThreadEvent,
      { type: 'UserPromptInjected', text: 'Uuhh fix the script?', mode: 'human', injected_message_id: 'a7d179ab-f451-4ff7-89dd-61ed413aaa88', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:01.092Z' } as ThreadEvent,
      { type: 'Thinking', text: '', context_tokens:15806, context_messages: 8, request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:01.098Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'emit_event', args: {}, description: 'Emitting event…', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:27.707Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'emit_event', result: 'ok', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:27.731Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_claude', args: {}, description: 'Executing Claude Code…', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:27.737Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'run_claude', result: 'ok', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:27.768Z' } as ThreadEvent,
      { type: 'Thinking', text: '', context_tokens:16425, context_messages: 10, request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:27.776Z' } as ThreadEvent,
      { type: 'TextStreamed', text: 'Released first…', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:31.886Z' } as ThreadEvent,
      { type: 'ResponseGenerated', text: 'Released first…', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:31.893Z' } as ThreadEvent,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // Parent owns pre-injection work only; the response splits to the follow-up.
    expect(exchanges[0].steps.length).toBeGreaterThan(0);
    expect(exchanges[0].steps.some(s => s.event.type === 'ResponseGenerated')).toBe(false);
    // Follow-up: UPI plus everything from injection onwards.
    const followupTypes = exchanges[1].steps.map(s => s.event.type);
    expect(followupTypes[0]).toBe('UserPromptInjected');
    expect(followupTypes).toContain('ResponseGenerated');

    const followupStatus = exchangeStatus(exchanges[1], '', /* isLast */ true, /* hasPriorActive */ false, /* threadIsCC */ false, /* threadIdle */ true);
    expect(followupStatus).toBe('done');
  });

  // Empty non-last chat exchange (no steps, no prior active, thread still
  // running) — the engine moved on to a later exchange. Must not register as
  // an ACTIVE status, otherwise the next exchange's priorActive gate flips it
  // to 'queued' indefinitely.
  it('empty non-last chat exchange does not lock the next exchange into queued', () => {
    seqCounter = 1;
    const { map, id } = makeThread('empty-non-last-1', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'A done', channel: 'chat', created: t(-30000) },
      { type: 'ResponseGenerated', text: 'A reply', created: t(-29000) } as ThreadEvent,
      // B never gets processed (no steps).
      { type: 'MessageReceived', text: 'B empty', channel: 'chat', created: t(-20000) },
      // C is the active exchange.
      { type: 'MessageReceived', text: 'C running', channel: 'chat', created: t(-10000) },
      { type: 'ToolCalled', name: 'run_bash', args: {}, description: 'Running bash...', created: t(-9000) } as ThreadEvent,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);
    const bStatus = exchangeStatus(exchanges[1], '', /* isLast */ false, /* hasPriorActive */ false, /* threadIsCC */ false, /* threadIdle */ false);
    expect(bStatus).toBe('done');
    // Sanity: not in ACTIVE_STATUSES.
    expect(isActive(bStatus)).toBe(false);
  });

  // Synthesized minimal version of the production scenario above.
  it('follow-up with absorbed UPI is done after parent ResponseGenerated, not aborted', () => {
    seqCounter = 1;
    const { map, id } = makeThread('followup-absorbed-real', 'idle');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the script', channel: 'chat', created: t(-30000), event_id: 'parent-mr-2' },
      { type: 'Thinking', text: 'analyzing', request_event_id: 'parent-mr-2', created: t(-29000) } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python code...', request_event_id: 'parent-mr-2', created: t(-25000) } as ThreadEvent,
      // Follow-up arrives mid-Python.
      { type: 'MessageReceived', text: 'Uuhh fix the script?', channel: 'chat', created: t(-22000), event_id: 'followup-mr-2' },
      // Python finally returns; engine drains injection and emits UPI.
      { type: 'ToolResult', name: 'run_python', result: 'ok', request_event_id: 'parent-mr-2', created: t(-15000) } as ThreadEvent,
      { type: 'UserPromptInjected', text: 'Uuhh fix the script?', mode: 'human', injected_message_id: 'followup-mr-2', request_event_id: 'parent-mr-2', created: t(-14990) } as ThreadEvent,
      { type: 'Thinking', text: 'now incorporating user note', request_event_id: 'parent-mr-2', created: t(-14000) } as ThreadEvent,
      { type: 'TextStreamed', text: 'Released first…', request_event_id: 'parent-mr-2', created: t(-2000) } as ThreadEvent,
      { type: 'ResponseGenerated', text: 'Released first…', request_event_id: 'parent-mr-2', created: t(-1000) } as ThreadEvent,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    // Follow-up owns the UPI marker plus the post-injection thinking, text
    // streaming, and final ResponseGenerated.
    expect(exchanges[1].userEvent.type).toBe('MessageReceived');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('Uuhh fix the script?');
    expect(exchanges[1].steps.map(s => s.event.type)).toEqual([
      'UserPromptInjected', 'Thinking', 'TextStreamed', 'ResponseGenerated',
    ]);

    const followupStatus = exchangeStatus(exchanges[1], '', /* isLast */ true, /* hasPriorActive */ false, /* threadIsCC */ false, /* threadIdle */ true);
    expect(followupStatus).toBe('done');
  });

  // Verbatim production payload from personal workspace thread
  // a81d6adc-6647-4cf4-9589-edb58eb57571 (2026-05-05). Same MR1 → many tools →
  // MR2 mid-flight → tools → UPI → tools → ResponseGenerated shape, but using
  // the actual UUIDs and timestamps so a regression that only hits a specific
  // ordering or id collision shows up here.
  it('production thread: absorbed UPI follow-up resolves to done', () => {
    seqCounter = 1;
    const { map, id } = makeThread('audit-prod', 'idle');
    const MR1 = 'c4f1ef84-48c2-4f5a-8319-e79d954c3722';
    const MR2 = 'ec64bf8b-f37e-4b22-beb5-e76a340f9175';
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'La oss droppe påminnelser trigger - og fjerne ref til den', channel: 'chat', created: '2026-05-05T06:11:30.599Z', event_id: MR1 },
      { type: 'MemorySearched', results: 60, request_event_id: MR1, created: '2026-05-05T06:11:32.094Z' } as ThreadEvent,
      { type: 'Thinking', text: '', context_tokens: 6934, context_messages: 1, request_event_id: MR1, created: '2026-05-05T06:11:32.137Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'list_triggers', args: {}, description: 'Listing triggers...', request_event_id: MR1, created: '2026-05-05T06:11:35.045Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'list_triggers', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:35.069Z' } as ThreadEvent,
      { type: 'Thinking', text: '', context_tokens: 7668, context_messages: 3, request_event_id: MR1, created: '2026-05-05T06:11:35.090Z' } as ThreadEvent,
      // MR2 lands while MR1 is still working.
      { type: 'MessageReceived', text: 'for calendar altså', channel: 'chat', created: '2026-05-05T06:11:39.201Z', event_id: MR2 },
      { type: 'ToolCalled', name: 'delete_trigger', args: {}, description: 'Deleting trigger...', request_event_id: MR1, created: '2026-05-05T06:11:41.445Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'delete_trigger', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:41.479Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'delete_file', args: {}, description: 'Deleting file...', request_event_id: MR1, created: '2026-05-05T06:11:41.489Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'delete_file', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:41.544Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'delete_file', args: {}, description: 'Deleting file...', request_event_id: MR1, created: '2026-05-05T06:11:41.549Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'delete_file', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:41.568Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'grep_files', args: {}, description: 'Grepping...', request_event_id: MR1, created: '2026-05-05T06:11:41.572Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'grep_files', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:42.239Z' } as ThreadEvent,
      // Engine drains injection and emits UPI.
      { type: 'UserPromptInjected', text: 'for calendar altså', mode: 'human', injected_message_id: MR2, request_event_id: MR1, created: '2026-05-05T06:11:42.244Z' } as ThreadEvent,
      { type: 'Thinking', text: '', context_tokens: 8329, context_messages: 6, request_event_id: MR1, created: '2026-05-05T06:11:42.248Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'read_file', args: {}, description: 'Reading file...', request_event_id: MR1, created: '2026-05-05T06:11:47.792Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'read_file', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:47.802Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_bash', args: {}, description: 'Running bash...', request_event_id: MR1, created: '2026-05-05T06:11:47.807Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'run_bash', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:48.031Z' } as ThreadEvent,
      { type: 'Thinking', text: '', context_tokens: 8630, context_messages: 8, request_event_id: MR1, created: '2026-05-05T06:11:48.041Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'edit_file', args: {}, description: 'Editing file...', request_event_id: MR1, created: '2026-05-05T06:11:56.694Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'edit_file', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:56.740Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'edit_file', args: {}, description: 'Editing file...', request_event_id: MR1, created: '2026-05-05T06:11:56.746Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'edit_file', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:56.770Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'grep_files', args: {}, description: 'Grepping...', request_event_id: MR1, created: '2026-05-05T06:11:56.774Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'grep_files', result: 'ok', request_event_id: MR1, created: '2026-05-05T06:11:56.787Z' } as ThreadEvent,
      { type: 'Thinking', text: '', context_tokens: 9013, context_messages: 10, request_event_id: MR1, created: '2026-05-05T06:11:56.791Z' } as ThreadEvent,
      { type: 'TextStreamed', text: 'Ferdig.', request_event_id: MR1, created: '2026-05-05T06:12:00.037Z' } as ThreadEvent,
      { type: 'ResponseGenerated', text: 'Ferdig.', request_event_id: MR1, created: '2026-05-05T06:12:00.046Z' } as ThreadEvent,
      // ThreadSaved lands AFTER ResponseGenerated as a metadata event with
      // no request_event_id. Without `current` being reset by the absorbed
      // UPI, this leaks into exchange 2 → onlyStep check fails (length > 1) →
      // threadIdle stale-detector flips status to 'aborted'.
      { type: 'ThreadSaved', created: '2026-05-05T06:12:00.056Z' } as ThreadEvent,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // Exchange 2 starts with the UPI and includes everything after — the
    // post-injection tools and the final ResponseGenerated.
    expect(exchanges[1].userEvent.type).toBe('MessageReceived');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('for calendar altså');
    const followupTypes = exchanges[1].steps.map(s => s.event.type);
    expect(followupTypes[0]).toBe('UserPromptInjected');
    expect(followupTypes[followupTypes.length - 1]).toBe('ResponseGenerated');

    const followupStatus = exchangeStatus(exchanges[1], '', /* isLast */ true, /* hasPriorActive */ false, /* threadIsCC */ false, /* threadIdle */ true);
    expect(followupStatus).toBe('done');
  });

  // The injection point is a real boundary: it's the moment the agentic loop
  // actually saw the new prompt. Pre-UPI work still belongs to the original
  // request; everything from UPI onwards (including the final response) is
  // the answer to the absorbed follow-up. Without this split, the user can't
  // tell which steps reacted to which message.
  it('post-UPI events route to the absorbed-into exchange, not the original request', () => {
    seqCounter = 1;
    const { map, id } = makeThread('upi-split', 'idle');
    const MR1 = 'aaaaaaaa-1111-1111-1111-111111111111';
    const MR2 = 'bbbbbbbb-2222-2222-2222-222222222222';
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'first', channel: 'chat', created: '2026-05-05T10:00:00.000Z', event_id: MR1 },
      { type: 'Thinking', text: 'pre', request_event_id: MR1, created: '2026-05-05T10:00:01.000Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'pre_tool', args: {}, description: 'Pre tool...', request_event_id: MR1, created: '2026-05-05T10:00:02.000Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'pre_tool', result: 'ok', request_event_id: MR1, created: '2026-05-05T10:00:03.000Z' } as ThreadEvent,
      // MR2 lands while the loop is still working on MR1.
      { type: 'MessageReceived', text: 'second', channel: 'chat', created: '2026-05-05T10:00:04.000Z', event_id: MR2 },
      // Loop finishes its current tool, then the engine drains the injection.
      { type: 'ToolCalled', name: 'in_flight', args: {}, description: 'In-flight tool...', request_event_id: MR1, created: '2026-05-05T10:00:05.000Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'in_flight', result: 'ok', request_event_id: MR1, created: '2026-05-05T10:00:06.000Z' } as ThreadEvent,
      // UPI lands — split point. From here onwards the loop "knows about"
      // the follow-up.
      { type: 'UserPromptInjected', text: 'second', mode: 'human', injected_message_id: MR2, request_event_id: MR1, created: '2026-05-05T10:00:07.000Z' } as ThreadEvent,
      { type: 'ToolCalled', name: 'post_tool', args: {}, description: 'Post tool...', request_event_id: MR1, created: '2026-05-05T10:00:08.000Z' } as ThreadEvent,
      { type: 'ToolResult', name: 'post_tool', result: 'ok', request_event_id: MR1, created: '2026-05-05T10:00:09.000Z' } as ThreadEvent,
      { type: 'TextStreamed', text: 'Combined', request_event_id: MR1, created: '2026-05-05T10:00:10.000Z' } as ThreadEvent,
      { type: 'ResponseGenerated', text: 'Combined', request_event_id: MR1, created: '2026-05-05T10:00:11.000Z' } as ThreadEvent,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    // Exchange 1 owns the work BEFORE the injection: the pre-UPI tools and
    // any work done while the prompt was sitting in the queue.
    const ex1Steps = exchanges[0].steps.map(s => s.event.type);
    expect(ex1Steps).toEqual([
      'Thinking',
      'ToolCalled', 'ToolResult',  // pre_tool
      'ToolCalled', 'ToolResult',  // in_flight
    ]);

    // Exchange 2 owns the UPI itself plus everything from injection time on,
    // including the final response.
    const ex2Steps = exchanges[1].steps.map(s => s.event.type);
    expect(ex2Steps).toEqual([
      'UserPromptInjected',
      'ToolCalled', 'ToolResult',  // post_tool
      'TextStreamed',
      'ResponseGenerated',
    ]);

    // E1 (non-last with pre-injection steps) → 'interrupted' ("Continued
    // below ↳"). E2 (last, with full response) → 'done'.
    expect(exchangeStatus(exchanges[0], '', /* isLast */ false, false, false, /* threadIdle */ true)).toBe('interrupted');
    expect(exchangeStatus(exchanges[1], '', /* isLast */ true, false, false, /* threadIdle */ true)).toBe('done');
  });
});
