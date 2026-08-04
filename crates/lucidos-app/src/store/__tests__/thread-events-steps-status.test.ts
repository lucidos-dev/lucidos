import { describe, it, expect } from 'vitest';
import { buildCCThread, makeThreadState } from './thread-events-helpers';
import { exchangeResponseEvents, exchangeSteps, fullCommandForCCTool, fullCommandForEngineTool, groupIntoExchanges, handleEvent, type StoredEvent, type ThreadEvent } from '../thread-events';
import type { StepOutcome } from '../types';

describe('UserPromptInjected in groupIntoExchanges', () => {
  it('UserPromptInjected starts a new exchange', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'do X' }],
      [2, { type: 'ToolCalled', name: 'web_search', args: {} }],
      [3, { type: 'ToolResult', name: 'web_search', result: 'ok' }],
      [4, { type: 'UserPromptInjected', text: 'actually do Y' }],
      [5, { type: 'TextStreamed', text: 'doing Y now' }],
      [6, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].userEvent).toEqual({ type: 'MessageReceived', text: 'do X' });
    expect(exchanges[0].steps).toHaveLength(2); // ToolCalled + ToolResult
    expect(exchanges[1].userEvent).toEqual({ type: 'UserPromptInjected', text: 'actually do Y' });
    expect(exchanges[1].steps).toHaveLength(2); // TextStreamed + ResponseGenerated
  });

  it('multiple injections create multiple exchanges', () => {
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'start' }],
      [2, { type: 'ToolCalled', name: 'search', args: {} }],
      [3, { type: 'UserPromptInjected', text: 'correction 1' }],
      [4, { type: 'ToolCalled', name: 'read', args: {} }],
      [5, { type: 'UserPromptInjected', text: 'correction 2' }],
      [6, { type: 'ResponseGenerated' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[1].userEvent.type).toBe('UserPromptInjected');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('correction 1');
    expect(exchanges[2].userEvent.type).toBe('UserPromptInjected');
    expect((exchanges[2].userEvent as { text: string }).text).toBe('correction 2');
  });

  it('UserPromptInjected with injected_message_id absorbs into matching MessageReceived exchange', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'first', _eventId: 'msg-1' }],
      [2, { type: 'ResponseGenerated', text: 'first response' }],
      [3, { type: 'MessageReceived', text: 'follow-up', _eventId: 'msg-2' }],
      [4, { type: 'UserPromptInjected', text: 'follow-up', injected_message_id: 'msg-2' }],
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
      'UserPromptInjected', 'TextStreamed', 'ResponseGenerated',
    ]);
  });

  it('UserPromptInjected without injected_message_id still starts its own exchange (legacy)', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'start', _eventId: 'msg-1' }],
      [2, { type: 'UserPromptInjected', text: 'inject' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('UserPromptInjected');
  });

  it('UserPromptInjected with injected_message_id and no matching MessageReceived falls back to its own exchange', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'start', _eventId: 'msg-1' }],
      [2, { type: 'UserPromptInjected', text: 'orphan', injected_message_id: 'missing' }],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('UserPromptInjected');
    expect((exchanges[1].userEvent as { text: string }).text).toBe('orphan');
  });

  // Real thread ab70366f: the chat agentic loop never re-anchors request_event_id,
  // so a message the user types mid-flight (emitted optimistically as its own
  // exchange) is only ingested via UserPromptInjected AFTER the agent asked — and
  // the user answered — a question. The absorb must re-anchor that queued exchange
  // to its ingestion point, otherwise its reply renders ABOVE the question card.
  it('queued mid-flight message ingested AFTER a question renders its reply below the question card', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'original', _eventId: 'req-orig' }],
      [2, { type: 'TextStreamed', text: 'working on original', request_event_id: 'req-orig' } as StoredEvent],
      // user types a second message mid-flight (fast-path emits it immediately)
      [3, { type: 'MessageReceived', text: 'cancel button broke', _eventId: 'msg-cancel' }],
      // agent (still on the original turn) asks a question before ingesting the queue
      [4, { type: 'UserQuestionAsked', question: 'pick one', tool_use_id: 'tuid-q', _eventId: 'uqa' } as StoredEvent],
      [5, { type: 'UserQuestionAnswered', tool_use_id: 'tuid-q' } as StoredEvent],
      // loop ingests the queued message — UPI carries the ORIGINAL turn's req id
      [6, { type: 'UserPromptInjected', text: 'cancel button broke', injected_message_id: 'msg-cancel', request_event_id: 'req-orig' } as StoredEvent],
      [7, { type: 'TextStreamed', text: 'analyzing the cancel bug', request_event_id: 'req-orig' } as StoredEvent],
      [8, { type: 'ResponseGenerated', text: 'done', request_event_id: 'req-orig' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const qIdx = exchanges.findIndex(e => e.userEvent.type === 'UserQuestionAsked');
    const cancelIdx = exchanges.findIndex(e =>
      e.userEvent.type === 'MessageReceived' &&
      (e.userEvent as { text?: string }).text === 'cancel button broke');
    expect(qIdx).toBeGreaterThanOrEqual(0);
    // The reply (the absorbed exchange) must render BELOW the question card.
    expect(cancelIdx).toBeGreaterThan(qIdx);
    expect(exchanges[cancelIdx].steps.map(s => s.event.type)).toContain('TextStreamed');
  });
});

// ===========================================================================
// Step spinner completion on finished exchanges
// ===========================================================================
describe('step completion — no eternal spinners', () => {
  it('parallel CC tool calls all resolve when results arrive', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    // Simulate: user message, session start, then parallel tool calls with results
    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'fix it' } as ThreadEvent, '2026-04-04T10:00:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-04T10:00:01Z');
    handleEvent(map, 'thread-1', 3, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'a.rs' } } as ThreadEvent, '2026-04-04T10:00:02Z');
    handleEvent(map, 'thread-1', 4, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'b.rs' } } as ThreadEvent, '2026-04-04T10:00:02Z');
    // Only ONE result arrives (CC parallel — result for first call lost)
    handleEvent(map, 'thread-1', 5, { type: 'CodingAgentToolResult', name: '', result: 'ok' } as ThreadEvent, '2026-04-04T10:00:03Z');
    // CC finishes
    handleEvent(map, 'thread-1', 6, { type: 'CodingAgentTextStreamed', text: 'Done!' } as ThreadEvent, '2026-04-04T10:00:04Z');
    handleEvent(map, 'thread-1', 7, { type: 'CodingAgentIdled' } as ThreadEvent, '2026-04-04T10:00:05Z');

    const exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    expect(exchanges).toHaveLength(1);

    const events = exchangeResponseEvents(exchanges[0]);
    const steps = events.filter(e => e.type === 'step');

    // Both steps must be completed (no spinner): the exchange is done, and
    // CodingAgentIdled is a CLEAN terminator, so the lost result resolves to a
    // checkmark rather than "did not finish".
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
    // NO ToolResult — session was killed
    handleEvent(map, 'thread-1', 4, { type: 'CodingAgentIdled' } as ThreadEvent, '2026-04-04T10:00:03Z');

    const exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    const events = exchangeResponseEvents(exchanges[0]);
    const steps = events.filter(e => e.type === 'step');

    expect(steps).toHaveLength(1);
    // Step must NOT show spinner on a completed exchange
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
    // CC idles then resumes with a new tool call (still in progress)
    handleEvent(map, 'thread-1', 5, { type: 'CodingAgentIdled' } as ThreadEvent, '2026-04-04T10:00:04Z');
    handleEvent(map, 'thread-1', 6, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, '2026-04-04T10:00:05Z');
    // No result yet — tool is actively running

    const exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    const events = exchangeResponseEvents(exchanges[0]);
    const steps = events.filter(e => e.type === 'step');

    // The last step should still show spinner — CC is actively working
    const lastStep = steps[steps.length - 1] as { outcome: StepOutcome };
    expect(lastStep.outcome).toBe('pending');
  });

  it('three parallel subagents resolve individually as results arrive (live streaming)', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'fix it' } as ThreadEvent, '2026-04-04T10:00:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, '2026-04-04T10:00:01Z');
    // Three parallel Agent launches
    handleEvent(map, 'thread-1', 3, { type: 'CodingAgentToolCalled', name: 'Agent', args: { prompt: 'task 1' } } as ThreadEvent, '2026-04-04T10:00:02Z');
    handleEvent(map, 'thread-1', 4, { type: 'CodingAgentToolCalled', name: 'Agent', args: { prompt: 'task 2' } } as ThreadEvent, '2026-04-04T10:00:02Z');
    handleEvent(map, 'thread-1', 5, { type: 'CodingAgentToolCalled', name: 'Agent', args: { prompt: 'task 3' } } as ThreadEvent, '2026-04-04T10:00:02Z');

    // First result arrives — should resolve exactly one step
    handleEvent(map, 'thread-1', 6, { type: 'CodingAgentToolResult', name: '', result: 'done 1' } as ThreadEvent, '2026-04-04T10:00:05Z');

    let exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    let events = exchangeResponseEvents(exchanges[0]);
    let steps = events.filter(e => e.type === 'step') as { outcome: StepOutcome }[];
    let resolved = steps.filter(s => s.outcome === 'success').length;
    let pending = steps.filter(s => s.outcome === 'pending').length;
    expect(resolved).toBe(1);
    expect(pending).toBe(2);

    // Second result
    handleEvent(map, 'thread-1', 7, { type: 'CodingAgentToolResult', name: '', result: 'done 2' } as ThreadEvent, '2026-04-04T10:00:06Z');
    exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    events = exchangeResponseEvents(exchanges[0]);
    steps = events.filter(e => e.type === 'step') as { outcome: StepOutcome }[];
    resolved = steps.filter(s => s.outcome === 'success').length;
    pending = steps.filter(s => s.outcome === 'pending').length;
    expect(resolved).toBe(2);
    expect(pending).toBe(1);

    // Third result
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
    expect(respSteps).toHaveLength(3);
    for (const step of respSteps) {
      expect((step as { outcome: StepOutcome }).outcome).toBe('success');
    }

    const steps = exchangeSteps(exchanges[0]);
    expect(steps).toHaveLength(3);
    for (const step of steps) {
      expect(step.outcome).toBe('success');
    }
  });

  it('parallel CC reads with same description: result pairs by tool_use_id, not visual order', () => {
    // Two CC `Read SKILL.md` calls run in parallel — same row label, different
    // paths, different tool_use_ids. The result for the first call arrives
    // before the second; the row that gets resolved must be the one whose
    // tool_use_id matches, not whichever pending row came last in the events.
    const events = new Map<number, ThreadEvent>([
      [1, { type: 'MessageReceived', text: 'audit skills', created: '2026-04-04T10:00:00Z' } as ThreadEvent],
      [2, { type: 'SessionStarted', session_id: 's1', created: '2026-04-04T10:00:01Z' } as ThreadEvent],
      [3, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: '/skills/grill-me/SKILL.md' }, tool_use_id: 'tu-A', created: '2026-04-04T10:00:02Z' } as ThreadEvent],
      [4, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: '/skills/superpowers/SKILL.md' }, tool_use_id: 'tu-B', created: '2026-04-04T10:00:02Z' } as ThreadEvent],
      // Only the result for the FIRST call has arrived so far.
      [5, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', tool_use_id: 'tu-A', created: '2026-04-04T10:00:03Z' } as ThreadEvent],
    ]);

    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);

    const respSteps = exchangeResponseEvents(exchanges[0]).filter(e => e.type === 'step') as { tool_use_id?: string; outcome: StepOutcome }[];
    expect(respSteps).toHaveLength(2);
    const stepA = respSteps.find(s => s.tool_use_id === 'tu-A');
    const stepB = respSteps.find(s => s.tool_use_id === 'tu-B');
    expect(stepA?.outcome).toBe('success');  // Row that got its result is done
    expect(stepB?.outcome).toBe('pending');  // Other row keeps spinning until its result arrives

    const steps = exchangeSteps(exchanges[0]) as { tool_use_id?: string; outcome: StepOutcome }[];
    expect(steps).toHaveLength(2);
    expect(steps.find(s => s.tool_use_id === 'tu-A')?.outcome).toBe('success');
    expect(steps.find(s => s.tool_use_id === 'tu-B')?.outcome).toBe('pending');
  });

  it('legacy CC events without tool_use_id fall back to backward-walk resolution', () => {
    // Stored events from before the tool_use_id field existed render with all
    // pending steps eventually resolved by description-based fallback alone.
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
    const respSteps = exchangeResponseEvents(exchanges[0]).filter(e => e.type === 'step') as { outcome: StepOutcome }[];
    expect(respSteps).toHaveLength(2);
    for (const step of respSteps) expect(step.outcome).toBe('success');
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

  it('exchangeSteps also resolves pending steps on completed exchange', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'fix it' } as ThreadEvent, '2026-04-04T10:00:00Z');
    handleEvent(map, 'thread-1', 2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, '2026-04-04T10:00:01Z');
    // No ToolResult, but exchange completed
    handleEvent(map, 'thread-1', 3, { type: 'ResponseGenerated' } as ThreadEvent, '2026-04-04T10:00:02Z');

    const exchanges = groupIntoExchanges(map.get('thread-1')!.events);
    const steps = exchangeSteps(exchanges[0]);

    expect(steps).toHaveLength(1);
    expect(steps[0].outcome).toBe('success');
  });
});

// ===========================================================================
// A tool call that never returned: the turn died mid-execution
// ===========================================================================
// Real thread, 2026-08-03: a Bash tool call at 07:44:41, the Claude Code
// process SIGKILLed at 07:44:43 before the tool produced anything, and the turn
// terminated at 07:44:44 with a red "Event stream error" card. The step row sat
// directly above that card with no check and no failure marker, reading as
// though it were still executing.
//
// The turn's terminator decides the verdict, and the two branches must stay
// split. A CLEAN end (ResponseGenerated / CodingAgentIdled) means the step did
// finish and merely lacks a recorded result, so a ✓ is honest. An UNCLEAN end
// (ResponseFailed / ResponseAborted / ResponseCanceled) means the step did NOT
// finish, so a ✓ would be a worse lie than the spinner it replaces.
// ===========================================================================

/** The reproduction's event shape: one tool call that got its result, one that
 *  was still running when `terminal` landed. */
function killedMidCall(terminal: Record<string, unknown>): Map<number, ThreadEvent> {
  return new Map<number, ThreadEvent>([
    [1, { type: 'MessageReceived', text: 'run the linter', created: '2026-08-03T07:44:30Z' } as ThreadEvent],
    [2, { type: 'SessionStarted', session_id: 's1', created: '2026-08-03T07:44:31Z' } as ThreadEvent],
    [3, { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: 'scripts/lib/webkit_shard.sh' }, tool_use_id: 'tu-done', created: '2026-08-03T07:44:35Z' } as ThreadEvent],
    [4, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', tool_use_id: 'tu-done', created: '2026-08-03T07:44:36Z' } as ThreadEvent],
    [5, { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'shellcheck scripts/lib/webkit_shard.sh' }, tool_use_id: 'tu-killed', created: '2026-08-03T07:44:41Z' } as ThreadEvent],
    // SIGKILL at 07:44:43: no CodingAgentToolResult for tu-killed, ever.
    [6, { ...terminal, created: '2026-08-03T07:44:44Z' } as unknown as ThreadEvent],
  ]);
}

/** Both projections of the first exchange, as `{ tool_use_id, outcome }` rows. */
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
        // The bug: 'pending' forever (an eternal spinner above a red error
        // card). The wrong fix: 'success' (a green check on a killed call).
        expect(killed?.outcome).toBe('unfinished');
        // The call that DID return keeps its checkmark, so the kill lands on
        // exactly one row and the user can see which.
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
        // A clean terminator means the tool DID finish and only its result
        // event is missing. Collapsing this branch into the unclean one would
        // put "did not finish" rows all over successful turns.
        expect(steps.find(s => s.tool_use_id === 'tu-killed')?.outcome).toBe('success');
        expect(steps.find(s => s.tool_use_id === 'tu-done')?.outcome).toBe('success');
      }
    });
  }

  it('a later same-request terminal wins: the recovered engine-restart turn keeps its checkmarks', () => {
    // Engine restart mid-turn: recovery emits ResponseAborted, the rerun re-uses
    // the original request_event_id and succeeds. `supersededAbortIndices`
    // already deflates the panel verdict to the success; the steps must agree.
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

// ===========================================================================
// Tool description from event (DRY: backend provides, frontend falls back)
// ===========================================================================
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
    // chars/4 estimate is wildly wrong for image-heavy turns (counts base64
    // bytes). The real input_tokens from the LLM provider arrives via a
    // ContextTokensMeasured event right after the response — both projections
    // (exchangeSteps for the secondary list, exchangeResponseEvents for the
    // inline chip in the chat view) must overwrite the displayed token count.
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
    // Bug: ThoughtStreamed was hardcoded to success: true, so the latest
    // "Thinking" step rendered with a ✓ checkmark while the agent was still
    // working. ThoughtStreamed marks "about to invoke the LLM with this
    // context size" — the Thinking step should stay as a spinner until the
    // LLM produces output (TextStreamed / ToolCalled), the next ThoughtStreamed
    // supersedes it, or the thread goes idle.
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, '2026-05-24T19:00:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed', text: 'Context: 100 tokens, 1 messages' } as ThreadEvent, '2026-05-24T19:00:01Z');
    // No terminator — LLM is still working

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const steps = exchangeSteps(exchanges[0], /* isLast */ true);
    expect(steps).toHaveLength(1);
    expect(steps[0].description).toBe('Thinking');
    expect(steps[0].outcome).toBe('pending');

    const respEvents = exchangeResponseEvents(exchanges[0]);
    const respSteps = respEvents.filter((e): e is Extract<typeof e, { type: 'step' }> => e.type === 'step');
    expect(respSteps).toHaveLength(1);
    expect(respSteps[0].outcome).toBe('pending');
  });

  it('Thinking resolves to ✓ when ToolCalled arrives', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, '2026-05-24T19:00:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed', text: 'Context: 100 tokens, 1 messages' } as ThreadEvent, '2026-05-24T19:00:01Z');
    handleEvent(map, 't', 3, { type: 'ToolCalled', name: 'run_bash', args: { command: 'ls' } } as ThreadEvent, '2026-05-24T19:00:02Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const steps = exchangeSteps(exchanges[0], /* isLast */ true);
    expect(steps[0].description).toBe('Thinking');
    expect(steps[0].outcome).toBe('success');
    expect(steps[1].outcome).toBe('pending'); // tool still pending

    const respSteps = exchangeResponseEvents(exchanges[0]).filter(
      (e): e is Extract<typeof e, { type: 'step' }> => e.type === 'step',
    );
    expect(respSteps[0].description).toBe('Thinking');
    expect(respSteps[0].outcome).toBe('success');
    expect(respSteps[1].outcome).toBe('pending');
  });

  it('Thinking resolves to ✓ when TextStreamed arrives', () => {
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, '2026-05-24T19:00:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed', text: 'Context: 100 tokens, 1 messages' } as ThreadEvent, '2026-05-24T19:00:01Z');
    handleEvent(map, 't', 3, { type: 'TextStreamed', text: 'hello' } as ThreadEvent, '2026-05-24T19:00:02Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const steps = exchangeSteps(exchanges[0], /* isLast */ true);
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
    const steps = exchangeSteps(exchanges[0], /* isLast */ true);
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
    // The engine middle-truncates run_bash descriptions to ~60 chars, marking the
    // cut with `…` and keeping the command's tail (`Running: cd /Users/…head -20...`).
    // The full command is preserved on the step so the UI can show it on mouseover.
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);
    const fullCmd = 'cd /Users/alex/IdeaProjects/lucidos && git log --oneline -50 | head -20';
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'show recent commits' } as ThreadEvent, '2026-04-30T10:00:00Z');
    handleEvent(map, 't', 2, { type: 'ToolCalled', name: 'run_bash', args: { command: fullCmd }, description: 'Running: cd /Users/alex/IdeaProjects/l…log --oneline -50 | head -20...' } as ThreadEvent, '2026-04-30T10:00:01Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    const respEvents = exchangeResponseEvents(exchanges[0]);
    const stepEvent = respEvents.find(e => e.type === 'step') as Extract<typeof respEvents[number], { type: 'step' }> | undefined;
    expect(stepEvent?.full).toBe(fullCmd);
  });

  it('exchangeResponseEvents stamps event.created on step events for the detail modal', () => {
    // Steps come from events; the modal needs the originating event's timestamp.
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
    // Both Codex protocols normalize the plan tool to {items: [{text, completed}]}
    // (exec emits it natively; app-server maps turn/plan/updated onto it).
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

  it('exchangeResponseEvents stamps full path on CodingAgentToolCalled steps for hover tooltip', () => {
    // Rust describe_cc_tool() shows only basename(file_path); the full path is
    // preserved on the step so the UI can show it on mouseover.
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['t', thread]]);
    const fullPath = '/Users/alex/IdeaProjects/lucidos/crates/lucidos-app/src/store/thread-events.ts';
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
    // Bug: user cancels mid-tool-call → MessageReceived starts a new exchange,
    // orphaning the ToolCalled in the previous exchange without a ToolResult or
    // completion event. The spinner persists forever.
    const thread = makeThreadState();
    const map = new Map([['t', thread]]);

    // Exchange 1: user asks, engine calls a tool
    handleEvent(map, 't', 1, { type: 'MessageReceived', text: 'find the file' } as ThreadEvent, '2026-04-13T19:30:00Z');
    handleEvent(map, 't', 2, { type: 'ThoughtStreamed' } as ThreadEvent, '2026-04-13T19:30:01Z');
    handleEvent(map, 't', 3, { type: 'ToolCalled', name: 'bash', args: { command: 'find . -name "Foo.tsx"' } } as ThreadEvent, '2026-04-13T19:30:02Z');
    // User sends a new message BEFORE the tool result arrives — starts Exchange 2
    handleEvent(map, 't', 4, { type: 'MessageReceived', text: 'stop, I found it' } as ThreadEvent, '2026-04-13T19:30:10Z');
    // The tool result and cancellation arrive AFTER the new message
    handleEvent(map, 't', 5, { type: 'ToolResult', name: 'bash', result: 'ok' } as ThreadEvent, '2026-04-13T19:30:15Z');
    handleEvent(map, 't', 6, { type: 'ResponseCanceled' } as ThreadEvent, '2026-04-13T19:30:15Z');

    const exchanges = groupIntoExchanges(map.get('t')!.events);
    // 1: original 'find the file' (with orphaned ToolCalled), 2: 'stop, I found it'
    // (with the late ToolResult + ResponseCanceled step), 3: cancel boundary panel.
    expect(exchanges).toHaveLength(3);
    expect(exchanges[2].userEvent.type).toBe('ResponseCanceled');

    // Verify the step IS pending when treated as the last exchange (the bug scenario)
    const stepsAsLast = exchangeSteps(exchanges[0], true);
    expect(stepsAsLast.filter(s => s.outcome === 'pending')).toHaveLength(1);

    // Exchange 1's ToolResult ended up in exchange 2. Cancel took the thread
    // idle, which is what resolves the orphaned spinner.
    const ex1Steps = exchangeSteps(exchanges[0], /* isLast */ false, /* threadIdle */ true);
    expect(ex1Steps.filter(s => s.outcome === 'pending')).toHaveLength(0);

    const ex1Events = exchangeResponseEvents(exchanges[0], /* isLast */ false, /* threadIdle */ true);
    const pendingEvents = ex1Events.filter(e => e.type === 'step' && (e as { outcome: StepOutcome }).outcome === 'pending');
    expect(pendingEvents).toHaveLength(0);
  });
});


// ===========================================================================
// exchangeStatus — CC follow-up scenarios (integration tests)
// ===========================================================================
// These tests simulate the full SSE→events→exchanges→status pipeline for CC
// follow-up messages. Each test builds events via handleEvent, groups them
// into exchanges, and asserts the user-visible status for each exchange.
//
// The bug: follow-up messages intermittently show "Aborted" status when the
// CC process exits during or after processing a follow-up.
// ===========================================================================


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
      // Exchange 1: initial request
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Read', args: {} } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:00:03Z' },
      { seq: 5, event: { type: 'CodingAgentTextStreamed', text: 'Done with initial analysis' } as ThreadEvent, created: '2026-04-12T10:00:04Z' },
      { seq: 6, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:05Z' },
      // Exchange 2: follow-up
      { seq: 7, event: { type: 'MessageReceived', text: 'now also fix the tests', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 8, event: { type: 'CodingAgentUserMessageSent', text: 'now also fix the tests' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 9, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      { seq: 10, event: { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
      { seq: 11, event: { type: 'CodingAgentTextStreamed', text: 'Tests fixed!' } as ThreadEvent, created: '2026-04-12T10:01:03Z' },
      { seq: 12, event: { type: 'CodingAgentIdled', has_changes: true } as ThreadEvent, created: '2026-04-12T10:01:04Z' },
    ]);
    expect(exchanges).toHaveLength(2);
    expect(statuses[0]).toBe('done');      // Exchange 1: completed normally
    expect(statuses[1]).toBe('done');      // Exchange 2: follow-up completed normally
  });

  it('CC follow-up without CodingAgentUserMessageSent (new data path) = both done', () => {
    const { statuses, exchanges } = buildCCThread([
      // Exchange 1
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:05Z' },
      // Exchange 2: follow-up with only MessageReceived (no CodingAgentUserMessageSent)
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
      // Exchange 1
      { seq: 1, event: { type: 'MessageReceived', text: 'first', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Exchange 2
      { seq: 4, event: { type: 'MessageReceived', text: 'second', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentUserMessageSent', text: 'second' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 6, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
      // Exchange 3
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
      // Exchange 1: completes normally
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Exchange 2: follow-up — CC crashes mid-work
      { seq: 4, event: { type: 'MessageReceived', text: 'also fix tests', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'CodingAgentUserMessageSent', text: 'also fix tests' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 6, event: { type: 'CodingAgentToolCalled', name: 'Edit', args: {} } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
      // CC process dies — safety-net ResponseAborted, also opens its own abort exchange.
      { seq: 7, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:01:02Z' },
    ]);
    // ResponseAborted dual-purpose: terminates exchange 2 AND opens a new
    // abort boundary exchange (the AbortPanel + Continue button surface).
    expect(exchanges).toHaveLength(3);
    expect(statuses[0]).toBe('done');      // Exchange 1 was already done
    expect(statuses[1]).toBe('aborted');   // Exchange 2 aborted by crash
    expect(exchanges[2].userEvent.type).toBe('ResponseAborted'); // boundary
  });

  it('CC crash on follow-up: ResponseAborted opens its own boundary exchange', () => {
    const { exchanges } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      { seq: 5, event: { type: 'ResponseAborted', text: '' } as ThreadEvent, created: '2026-04-12T10:01:01Z' },
    ]);
    // 1: idle, 2: aborted follow-up, 3: ResponseAborted boundary
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
      // Exchange 1: normal
      { seq: 1, event: { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Exchange 2: follow-up sent, but CC was exiting — message lost
      { seq: 4, event: { type: 'MessageReceived', text: 'follow-up that got lost', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:01:00Z' },
      // Backend drains lost follow-ups → single ResponseAborted (also opens
      // its own abort boundary exchange).
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
    // Engine flow when CC dies before producing a Result for a follow-up:
    // the run_session safety net emits ResponseAborted, then the post-loop
    // emits SessionEnded(completed). The exchange reads as aborted because
    // ResponseAborted set isAborted=true; SessionEnded(completed) is just
    // the normal lifecycle terminator that follows.
    const { statuses } = buildCCThread([
      { seq: 1, event: { type: 'MessageReceived', text: 'fix', channel: 'claude_code' } as ThreadEvent, created: '2026-04-12T10:00:00Z' },
      { seq: 2, event: { type: 'SessionStarted', session_id: 's1' } as ThreadEvent, created: '2026-04-12T10:00:01Z' },
      { seq: 3, event: { type: 'CodingAgentIdled', has_changes: false } as ThreadEvent, created: '2026-04-12T10:00:02Z' },
      // Follow-up — CC starts working, dies without Result, safety net fires
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
    // Both deltas coalesced onto the one Thinking step, in order.
    expect(thinking!.thinkingText).toBe('Let me check the tokens.');
    // Visible text resolved the Thinking step (no dangling spinner).
    expect(thinking!.outcome).toBe('success');
  });

  it('opens a Thinking step from reasoning when no CodingAgentPromptSent fired (resumed initial prompt)', () => {
    const thread = makeThreadState();
    thread.meta.channel = 'claude_code';
    const map = new Map([['thread-1', thread]]);

    // Resume-after-cancel: the follow-up is the resumed session's initial prompt,
    // so NO CodingAgentPromptSent is emitted — reasoning is the first activity.
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
    // Still streaming — the step is live (spinner), not resolved.
    expect(thinking!.outcome).toBe('pending');
  });
});

