import { describe, it, expect, beforeEach } from 'vitest';
import { getExchanges, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { exchangeResponseEvents, exchangeStatus, exchangeSteps, hasRenderableResponseContent, type ThreadEvent } from '../thread-events';
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
      // Exchange 1: initial Claude Code session completes
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
    expect(status).toBe('coding-agent-working');
  });

  it('stale_resume only (before retry SessionStarted arrives) is not aborted', () => {
    resetSeqCounter();
    const { map, id } = makeThread('stale-resume-2', 'running');

    insertEvents(map, id, [
      // Exchange 1: initial Claude Code session completes
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
    resetSeqCounter();
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
    resetSeqCounter();
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
    // displaySection should be 'current', not 'archive'
    expect(displaySection(thread.meta.section, thread.meta.status, thread.meta.saved, thread.meta.activeChildrenCount > 0, thread.meta.codingAgentProposed, false)).toBe('current');
  });
});

// ---------------------------------------------------------------------------
// Stale exchange recovery — incomplete exchanges after engine crash/lid close
// ---------------------------------------------------------------------------

describe('stale exchange recovery (incomplete last exchange)', () => {
  const t = (ms: number) => new Date(Date.now() + ms).toISOString();

  it('chat thread: last exchange with ToolCalled but no terminal event shows aborted when thread is idle', () => {
    resetSeqCounter();
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
    const pendingSteps = steps.filter(s => s.outcome === 'pending');
    expect(pendingSteps).toHaveLength(0);
  });

  it('chat thread: last exchange with only TextStreamed (no tools) shows aborted when thread is idle', () => {
    resetSeqCounter();
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
    resetSeqCounter();
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
    resetSeqCounter();
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
    resetSeqCounter();
    const { map, id } = makeThread('upi-folded-1', 'idle');

    insertEvents(map, id, [
      // Prior message — agentic loop is mid-stream when the follow-up arrives.
      { type: 'MessageReceived', text: 'ferdig', channel: 'chat', created: t(-300000), event_id: 'msg-A' },
      { type: 'ThoughtStreamed', text: 'analyzing', request_event_id: 'msg-A', created: t(-299000) } as ThreadEvent,
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
    // → 'interrupted' ("Done ↳"). The response continues in the
    // follow-up exchange after the UPI absorbed the new prompt.
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect((exchanges[0].userEvent as { text: string }).text).toBe('ferdig');
    expect(exchanges[0].steps.map(s => s.event.type)).toEqual([
      'ThoughtStreamed', 'ToolCalled', 'ToolResult', 'ToolCalled', 'ToolResult',
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
// terminal CC event, so the status falls through to 'coding-agent-working' forever.
// ---------------------------------------------------------------------------
describe('CC SessionEnded(changes_proposed) without preceding CodingAgentIdled', () => {
  const t = (offset: number) => new Date(Date.now() + offset).toISOString();

  it('treats SessionEnded(changes_proposed) as terminal even without CodingAgentIdled', () => {
    resetSeqCounter();
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
    // Must NOT be 'coding-agent-working' — SessionEnded means the agent is no longer running.
    expect(status).not.toBe('coding-agent-working');
    // SessionEnded with a normal lifecycle reason is terminal → 'done'.
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
    resetSeqCounter();
    const { map, id } = makeThread('parent-midflight-1', 'running');

    insertEvents(map, id, [
      // Parent message — agentic loop starts processing.
      { type: 'MessageReceived', text: 'fix the script', channel: 'chat', created: t(-30000), event_id: 'parent-mr' },
      { type: 'ThoughtStreamed', text: 'analyzing', request_event_id: 'parent-mr', created: t(-29000) } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python code...', request_event_id: 'parent-mr', created: t(-25000) } as ThreadEvent,
      { type: 'ToolResult', name: 'run_python', result: 'ok', request_event_id: 'parent-mr', created: t(-24000) } as ThreadEvent,
      { type: 'ThoughtStreamed', text: 'now python again', request_event_id: 'parent-mr', created: t(-22000) } as ThreadEvent,
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
    const events = exchangeResponseEvents(exchanges[0], /* isLast */ false);
    const pythonSteps = events.filter(e => e.type === 'step' && /python/i.test((e as { description?: string }).description ?? ''));
    expect(pythonSteps).toHaveLength(2);
    const lastPython = pythonSteps[pythonSteps.length - 1] as { outcome: StepOutcome };
    expect(lastPython.outcome).toBe('pending'); // spinner, not ✓
  });

  // Verbatim event shape from a production thread: parent emits two
  // run_python tool calls, follow-up MR lands while the second is in flight,
  // engine drains injection and emits UPI absorbing into the follow-up,
  // ResponseGenerated routes back to the parent via request_event_id.
  it('production-style absorbed UPI: follow-up status is done, not aborted', () => {
    resetSeqCounter();
    const { map, id } = makeThread('upi-prod', 'idle');
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
      // User sends follow-up while the second run_python is executing.
      { type: 'MessageReceived', text: 'Uuhh fix the script?', channel: 'chat', created: '2026-05-04T11:52:38.205Z', event_id: 'a7d179ab-f451-4ff7-89dd-61ed413aaa88' },
      { type: 'ToolResult', name: 'run_python', result: 'ok', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:01.080Z' } as ThreadEvent,
      { type: 'UserPromptInjected', text: 'Uuhh fix the script?', mode: 'human', injected_message_id: 'a7d179ab-f451-4ff7-89dd-61ed413aaa88', request_event_id: '1f2d02af-55ca-42c1-b52a-4e2067548d31', created: '2026-05-04T11:53:01.092Z' } as ThreadEvent,
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
    resetSeqCounter();
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
    resetSeqCounter();
    const { map, id } = makeThread('followup-absorbed-real', 'idle');

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the script', channel: 'chat', created: t(-30000), event_id: 'parent-mr-2' },
      { type: 'ThoughtStreamed', text: 'analyzing', request_event_id: 'parent-mr-2', created: t(-29000) } as ThreadEvent,
      { type: 'ToolCalled', name: 'run_python', args: {}, description: 'Running Python code...', request_event_id: 'parent-mr-2', created: t(-25000) } as ThreadEvent,
      // Follow-up arrives mid-Python.
      { type: 'MessageReceived', text: 'Uuhh fix the script?', channel: 'chat', created: t(-22000), event_id: 'followup-mr-2' },
      // Python finally returns; engine drains injection and emits UPI.
      { type: 'ToolResult', name: 'run_python', result: 'ok', request_event_id: 'parent-mr-2', created: t(-15000) } as ThreadEvent,
      { type: 'UserPromptInjected', text: 'Uuhh fix the script?', mode: 'human', injected_message_id: 'followup-mr-2', request_event_id: 'parent-mr-2', created: t(-14990) } as ThreadEvent,
      { type: 'ThoughtStreamed', text: 'now incorporating user note', request_event_id: 'parent-mr-2', created: t(-14000) } as ThreadEvent,
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
      'UserPromptInjected', 'ThoughtStreamed', 'TextStreamed', 'ResponseGenerated',
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
    resetSeqCounter();
    const { map, id } = makeThread('upi-split', 'idle');
    const MR1 = 'aaaaaaaa-1111-1111-1111-111111111111';
    const MR2 = 'bbbbbbbb-2222-2222-2222-222222222222';
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'first', channel: 'chat', created: '2026-05-05T10:00:00.000Z', event_id: MR1 },
      { type: 'ThoughtStreamed', text: 'pre', request_event_id: MR1, created: '2026-05-05T10:00:01.000Z' } as ThreadEvent,
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
      'ThoughtStreamed',
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

    // E1 (non-last with pre-injection steps) → 'interrupted' ("Done ↳").
    // E2 (last, with full response) → 'done'.
    expect(exchangeStatus(exchanges[0], '', /* isLast */ false, false, false, /* threadIdle */ true)).toBe('interrupted');
    expect(exchangeStatus(exchanges[1], '', /* isLast */ true, false, false, /* threadIdle */ true)).toBe('done');
  });
});

// ---------------------------------------------------------------------------
// CC mid-flight injection followed by AskUserQuestion: when the engine
// emits CodingAgentPromptSent for a queued user message but CC pauses on a
// new AskUserQuestion before consuming it, the prior exchange's Thinking
// step has nothing to resolve it (CC's resume events will attach to the
// new UserQuestionAsked exchange, not the queued message's exchange).
// `waiting_for_user_answer` must therefore behave like `idle` for the
// trailing-spinner cleanup.
// ---------------------------------------------------------------------------
describe('CC waiting_for_user_answer: trailing Thinking cleanup in non-last exchange', () => {
  it('strips stranded CodingAgentPromptSent Thinking when CC paused on AskUserQuestion', () => {
    const { map, id } = makeThread('cc-strand-1', 'idle');

    insertEvents(map, id, [
      // E1: user starts CC turn that ends with an AskUserQuestion.
      { type: 'MessageReceived', text: 'help with auth design', channel: 'claude_code', mode: 'human' } as any,
      { type: 'SessionStarted', session_id: 'cc-1', branch: 'claude-code/test' },
      { type: 'CodingAgentTextStreamed', text: 'Initial analysis…' },
      // E2 starter: AskUserQuestion. CC paused.
      { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: 'cc-1', question: 'Pick approach:', options: [{ id: 'opt-0', label: 'A' }] },
      { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'opt-0' } },
      { type: 'CodingAgentPromptSent', text: '' },
      { type: 'CodingAgentToolResult', name: '', result: 'opt-0', tool_use_id: 'tu-1' },
      // CC starts streaming its answer to E2's question…
      { type: 'CodingAgentTextStreamed', text: 'Pipeline accepted. Next:' },
      // …but a follow-up user message lands mid-flight (E3 starter).
      // The chat fast-path orders MessageReceived BEFORE the queued
      // CodingAgentPromptSent (run_session.rs flushes text first, then sends
      // input, then emits CodingAgentPromptSent — the engine's emit comes
      // after the MR by 10s of milliseconds).
      { type: 'MessageReceived', text: 'A provider can impl just some layers?', channel: 'claude_code', mode: 'human' } as any,
      { type: 'CodingAgentPromptSent', text: 'A provider can impl just some layers?' },
      // CC, still on its prior iteration, asks a NEW AskUserQuestion (E4
      // starter) before ever consuming the queued message. CC will
      // process the queued message AFTER the user answers this new
      // question — those future events will attach to E4, not E3, so
      // E3's CodingAgentPromptSent Thinking is stranded.
      { type: 'UserQuestionAsked', tool_use_id: 'tu-2', cc_session_id: 'cc-1', question: 'Next decision:', options: [{ id: 'opt-0', label: 'X' }] },
    ]);

    // UserQuestionAsked moves the thread to waiting_for_user_answer, which
    // isThreadQuiescent treats as an output-paused state for spinner cleanup.
    expect(map.get(id)!.meta.status).toBe('waiting_for_user_answer');
    const threadIdle = isThreadQuiescent(map.get(id)!.meta.status);
    expect(threadIdle).toBe(true);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(4);
    expect(exchanges[2].userEvent.type).toBe('MessageReceived');
    expect(exchanges[3].userEvent.type).toBe('UserQuestionAsked');

    // E2 (AskUserQuestion exchange) and E3 (mid-flight MR) both have a
    // stranded resume-marker / queued-prompt Thinking from CodingAgentPromptSent.
    // Both are non-last and must be cleaned up.
    const isPendingThinking = (e: { type: string; description?: string; outcome?: StepOutcome }) =>
      e.type === 'step' && e.description === 'Thinking' && e.outcome === 'pending';

    const e2Events = exchangeResponseEvents(exchanges[1], /* isLast */ false, threadIdle);
    expect(e2Events.filter(isPendingThinking)).toHaveLength(0);

    const e3Events = exchangeResponseEvents(exchanges[2], /* isLast */ false, threadIdle);
    expect(e3Events.filter(isPendingThinking)).toHaveLength(0);

    // E3 is stranded for a reason the fold can see without any thread-level
    // signal: raising E4's question handed the turn to it, so E3 carries
    // `continuationMoved` and its marker is finalized even while the thread is
    // still reported as running. Quiescence remains the cleanup for exchanges
    // the fold does NOT mark (E2 above).
    expect(exchanges[2].continuationMoved).toBe(true);
    const e3WhileRunning = exchangeResponseEvents(exchanges[2], /* isLast */ false, /* threadIdle */ false);
    expect(e3WhileRunning.filter(isPendingThinking)).toHaveLength(0);
  });
});

// Regression (2026-08-01): a coding-agent turn whose terminator never landed
// read "Working" forever. `exchangeStatus`'s `if (isCC)` branch returned
// 'coding-agent-working' BEFORE the threadIdle stale detector could fire, so a
// CC exchange with steps and no terminal spun a live-looking spinner on a
// subprocess that no longer existed.
//
// The engine-side cause was a teardown that Esc'd a session parked on an
// unanswered AskUserQuestion (fixed separately): CC recorded a rejection the
// user never made, raced past the question, and the terminal that would have
// followed was suppressed. This suite pins the CLIENT half, which must hold
// however the turn lost its terminator.
// See docs/plans/2026-08-01-preserve-question-parked-session-through-teardown.md
describe('coding-agent turn with no terminator must not read Working forever', () => {
  const now = Date.now();
  const t = (offset: number) => new Date(now + offset).toISOString();

  /** The reproduced shape: question asked, agent raced past it, no
   *  CodingAgentIdled / SessionEnded / ResponseAborted ever.
   *
   *  `trailingText` is the second half of what a teardown-Esc actually emitted
   *  (the rejection tool result, then a `"\n\n"` continuation). Both shapes
   *  must behave identically: `CodingAgentToolResult` alone is in
   *  `QUESTION_OVERTAKEN_STEP_TYPES` (so the card is already struck through and
   *  disabled) but is NOT one of the three types that clear `isWaitingForAnswer`
   *  in the status walk, so before the fix the tool-result-only shape read
   *  "Needs your answer" over dead buttons instead of "Working" forever. Same
   *  dead end, different label. */
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
      // Teardown Esc'd the pending AskUserQuestion: CC reports a rejection the
      // user never made. No terminal follows.
      { type: 'CodingAgentToolResult', name: '', result: 'The user doesn\'t want to proceed with this tool use.', created: t(-200000) },
      ...(trailingText
        ? [{ type: 'CodingAgentTextStreamed' as const, text: '\n\n', created: t(-199000) }]
        : []),
    ]);
    return getExchanges(map, id);
  };

  it('reads aborted once the thread has settled, not coding-agent-working', () => {
    resetSeqCounter();
    const exchanges = seedOvertakenQuestion('cc-no-terminator-1', 'idle');
    const last = exchanges[exchanges.length - 1];

    const status = exchangeStatus(last, '', true, false, /* threadIsCC */ true, /* threadIdle */ true);
    expect(status).toBe('aborted');
    expect(isActive(status)).toBe(false);
  });

  it('still reads coding-agent-working while the thread is genuinely running', () => {
    resetSeqCounter();
    const exchanges = seedOvertakenQuestion('cc-no-terminator-2', 'running');
    const last = exchanges[exchanges.length - 1];

    const status = exchangeStatus(last, '', true, false, /* threadIsCC */ true, /* threadIdle */ false);
    expect(status).toBe('coding-agent-working');
  });

  it('does not fire during the answer-to-resume gap (threadAwaitingAnswer)', () => {
    resetSeqCounter();
    const exchanges = seedOvertakenQuestion('cc-no-terminator-3', 'waiting');
    const last = exchanges[exchanges.length - 1];

    const status = exchangeStatus(
      last, '', true, false, /* threadIsCC */ true, /* threadIdle */ true, /* threadAwaitingAnswer */ true,
    );
    expect(status).not.toBe('aborted');
    expect(status).toBe('coding-agent-working');
  });

  /** The exact Esc shape, with no trailing text to rescue it. The card is
   *  already overtaken (struck through, buttons disabled), so claiming it
   *  "Needs your answer" is a dead end the user cannot act on. */
  it('a lone rejected tool result reads aborted, not "Needs your answer"', () => {
    resetSeqCounter();
    const exchanges = seedOvertakenQuestion('cc-no-terminator-4', 'idle', /* trailingText */ false);
    const last = exchanges[exchanges.length - 1];
    expect(last.questionOvertaken).toBe(true);

    const status = exchangeStatus(last, '', true, false, /* threadIsCC */ true, /* threadIdle */ true);
    expect(status).toBe('aborted');
  });

  /** The complement: a question nothing has raced past is still answerable,
   *  and must keep reading "Needs your answer" even on a settled thread. */
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

// Regression (2026-08-06): the same "Working forever" class, reached through the
// ABORT BOUNDARY instead of through a raced question, and reported as "Says its
// working before it has restarted".
//
// A `ResponseAborted` is dual-purpose in the fold: it terminates the originating
// exchange AND opens a boundary exchange of its own. The subprocess the teardown
// just Esc'd keeps draining for a few more milliseconds, and because CC events
// fold chronologically rather than by request id, that drain lands in the
// boundary as steps. Reproduced from a real event log: the abort
// at .145, the Esc rejection at .186, a `"\n\n"` flush at .197, and the resume 25
// seconds later.
//
// The stale detector above should have caught it, but it is gated on
// `threadIdle`, and a switch teardown does not leave the thread in a quiescent
// status: it settles at 'paused', or at 'waiting' when a change was already
// proposed, which the drain then revives to 'running'. So the boundary fell
// through to `isCC && !isStale` and shimmered "Working" while the engine was
// down. The fix does not widen quiescence (see the live-turn case at the bottom
// for why that would misfire); it lets the switch fingerprint supply the
// quiescence itself, since an engine that is going down cannot be working.
// See docs/plans/2026-08-06-no-working-label-while-nothing-is-running.md
describe('an abort boundary must not read Working while the engine is down', () => {
  const now = Date.now();
  const t = (offset: number) => new Date(now + offset).toISOString();
  const device = { kind: 'device', device_id: 'd1', label: 'My MacBook' } as const;

  /** The reproduced teardown: a turn in flight, the boundary abort, then the
   *  dying subprocess's last two events landing under it.
   *
   *  `proposed` seeds a change, which is what makes the same abort settle at
   *  'waiting' instead of 'paused' and then get revived to 'running' by the
   *  drain. The rendering must not depend on which of those it lands on. */
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
      // The drain, 41ms and 52ms after the abort in the real log.
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

      // The thread's own status is NOT quiescent in either shape, which is
      // exactly why the fix cannot lean on it.
      const threadIdle = isRenderedThreadIdle(map.get(id));
      expect(threadIdle).toBe(false);
      expect(isThreadQuiescent(map.get(id)!.meta.status)).toBe(false);

      const status = exchangeStatus(boundary, '', true, false, /* threadIsCC */ true, threadIdle);
      expect(isActive(status)).toBe(false);
      expect(status).toBe('aborted');
    });
  }

  /** The drain is not renderable content, so the boundary shows no response
   *  panel at all: the transcript is the "Paused by restart" panel alone. */
  it('has nothing renderable to show under the boundary', () => {
    resetSeqCounter();
    const { exchanges } = seedTeardownBoundary('cc-teardown-empty');
    const boundary = exchanges[exchanges.length - 1];

    const events = exchangeResponseEvents(boundary, true, false);
    expect(events.length).toBeGreaterThan(0);
    expect(hasRenderableResponseContent(events)).toBe(false);
  });

  /** The complement, and the case commit 3da5620eb exists for: a REAL turn runs
   *  under an abort boundary. A `safety_net` abort fires on a turn the watchdog
   *  thought was stuck, the loop keeps going, and two minutes of work lands here
   *  (real thread ebc787a4). That boundary is not a switch teardown, and its
   *  turn carries no start event at all, so nothing but the live events says it
   *  is alive. It must still read "Working", and then "Done" at its terminal.
   *
   *  This is also why quiescence was not widened to cover `failed`: the
   *  `safety_net` abort settles this very thread at `failed` and
   *  `preserving_verdict` pins it there for the whole live turn. */
  it('still reads Working for a live turn under a non-switch boundary', () => {
    resetSeqCounter();
    const { map, id } = makeThread('safety-net-live', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'close out the pipeline', created: t(-300000) },
      { type: 'ToolCalled', name: 'run_bash', args: { command: 'ls' }, created: t(-299000) },
      { type: 'ResponseAborted', cause: 'safety_net', actor: { kind: 'system' }, created: t(-290000) },
      // The loop never noticed: a full turn lands under the boundary, with no
      // start event of its own.
      { type: 'ThoughtStreamed', text: 'Context: 63064 tokens', created: t(-280000) },
      { type: 'ToolCalled', name: 'edit_file', args: { path: 'a.md' }, created: t(-279000) },
      { type: 'ToolResult', name: 'edit_file', result: 'ok', created: t(-278000) },
      { type: 'TextStreamed', text: 'Cut and carried over.', created: t(-277000) },
    ] as any);

    expect(map.get(id)!.meta.status).toBe('failed');
    const live = getExchanges(map, id).slice(-1)[0];
    expect(live.userEvent.type).toBe('ResponseAborted');
    // Quiescent by status would call this crashed; the switch fingerprint does not.
    expect(exchangeStatus(live, '', true, false, /* threadIsCC */ false, false)).toBe('streaming');
    expect(hasRenderableResponseContent(exchangeResponseEvents(live, true, false))).toBe(true);

    insertEvents(map, id, [{ type: 'ResponseGenerated', text: 'done', created: t(-276000) }] as any);
    const finished = getExchanges(map, id).slice(-1)[0];
    expect(exchangeStatus(finished, '', true, false, false, false)).toBe('done');
  });
});
