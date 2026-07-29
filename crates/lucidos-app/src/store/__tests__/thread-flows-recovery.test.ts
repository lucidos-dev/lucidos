import { describe, it, expect, beforeEach } from 'vitest';
import { getExchanges, getLabel, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { exchangeResponseEvents, exchangeResponseText, exchangeStatus, exchangeSteps, exchangeUserChannel, exchangeUserMessage, isEmptyContinuedExchange, resumeEngineNote, type ThreadEvent } from '../thread-events';
import { getEventToggleState } from '../event-rendering';

beforeEach(resetSeqCounter);

describe('Flow: Interrupted exchanges', () => {
  it('CC exchange followed by another shows "Done"', () => {
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
    expect(getLabel(exchanges[0], '', false)).toBe('Done');
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

  it('CC follow-up exchange (no own SessionStarted) interrupted shows "Done"', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      // Exchange 1: initial CC request, completed normally
      { type: 'MessageReceived', text: 'Center the separator', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: '2026-01-01T00:00:03Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:04Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:05Z' },
      // Exchange 2: follow-up in same Claude Code session (no SessionStarted), interrupted
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
    // "Done ↳" header, same as it does for empty 'done' exchanges.
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
    expect(getLabel(exchanges[0], '', false, false, true)).toBe('Done');
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
      { type: 'ThoughtStreamed', text: '', created: '2026-01-01T00:00:02Z' },
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
      { type: 'ThoughtStreamed', request_event_id: mr1Id, text: 'thinking...', created: '2026-01-01T00:00:03Z' } as any,
      { type: 'ToolCalled', name: 'Read', args: {}, request_event_id: mr1Id, created: '2026-01-01T00:00:10Z' } as any,
      { type: 'ToolResult', name: 'Read', result: 'ok', request_event_id: mr1Id, created: '2026-01-01T00:00:11Z' } as any,
      { type: 'MessageReceived', text: 'Also use more horizontal space...', created: '2026-01-01T00:00:30Z', event_id: mr2Id },
      // UPI absorbs into MR2's exchange and sets reqIdRedirect[mr1Id]=E2 —
      // subsequent req_id=mr1Id events redirect to E2.
      { type: 'UserPromptInjected', text: 'Also use more horizontal space...', mode: 'human',
        request_event_id: mr1Id, injected_message_id: mr2Id, created: '2026-01-01T00:01:00Z' } as any,
      { type: 'ThoughtStreamed', request_event_id: mr1Id, text: 'more thinking', created: '2026-01-01T00:01:05Z' } as any,
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // E1 (not last) should NOT be 'streaming' / Working — the user has moved on.
    // It should be 'interrupted' (label "Done ↳") matching the
    // existing CC pattern for mid-work interruptions.
    expect(exchangeStatus(exchanges[0], '', false)).toBe('interrupted');
    expect(getLabel(exchanges[0], '', false)).toBe('Done');
    // E2 (last) — still actively processing → Working
    expect(exchangeStatus(exchanges[1], '', true)).toBe('streaming');
    expect(getLabel(exchanges[1], '', true)).toBe('Working');
  });
});

// ---------------------------------------------------------------------------
// Flow 12: Recovery Claude Code session
// ---------------------------------------------------------------------------
describe('Flow: Recovery Claude Code session', () => {
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
// Flow 12b: Recovery via ContinuationStarted (auto-recovery of interrupted sessions)
// ---------------------------------------------------------------------------
describe('Flow: ContinuationStarted recovery', () => {
  it('ContinuationStarted acts as exchange boundary, thread shows CC content', () => {
    const { map, id } = makeThread();

    // Recovery session: ContinuationStarted for auto-recovered interrupted sessions
    insertEvents(map, id, [
      { type: 'ContinuationStarted', branch: 'claude-code/20260318-122816' },
      { type: 'SessionStarted', session_id: 'cc-1', branch: 'claude-code/20260318-122816' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' },
      { type: 'CodingAgentTextStreamed', text: 'Reviewed and continuing.' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    // ContinuationStarted is the user event (system-initiated, no user message)
    expect(exchanges[0].userEvent.type).toBe('ContinuationStarted');
    // Engine-initiated continuation (no actor / engine actor) — the "after
    // engine restart" wording is only honest when the engine itself drove
    // the resume (real restart recovery, watchdog auto-recovery). The
    // user-clicked-Continue case is covered by the next test.
    expect(exchangeUserMessage(exchanges[0])).toBe('Resumed after engine restart');
    expect(exchangeUserChannel(exchanges[0])).toBe('claude_code');
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    expect(exchangeResponseText(exchanges[0])).toContain('Reviewed and continuing.');
  });

  it('ContinuationStarted from user-clicked Continue does NOT say "engine restart"', () => {
    // Regression: when the user clicked Continue on a safety-net-aborted
    // thread, the engine emitted ContinuationStarted with actor.kind=device,
    // but the frontend hardcoded "Resumed after engine restart" — lying
    // about what happened (the engine was never restarted). The summary
    // must reflect the user's action, not the engine-restart case it
    // shares an event type with.
    const { map, id } = makeThread();
    insertEvents(map, id, [
      {
        type: 'ContinuationStarted',
        branch: 'claude-code/20260318-122816',
        actor: { kind: 'device', device_id: 'd-1', label: 'My Mac' },
      },
      { type: 'SessionStarted', session_id: 'cc-2', branch: 'claude-code/20260318-122816' },
      { type: 'CodingAgentTextStreamed', text: 'Continuing.' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].userEvent.type).toBe('ContinuationStarted');
    expect(exchangeUserMessage(exchanges[0])).toBe('Continued the response');
    expect(exchangeUserMessage(exchanges[0])).not.toContain('engine restart');
  });

  it('ContinuationStarted from api actor renders as user continuation too', () => {
    // `kind: 'api'` defaults to human mode (non-browser human path —
    // CLI / SDK caller).
    const { map, id } = makeThread();
    insertEvents(map, id, [
      {
        type: 'ContinuationStarted',
        branch: 'claude-code/20260318',
        actor: { kind: 'api', user_agent: 'lucidos-cli/0.1' },
      },
      { type: 'SessionStarted', session_id: 'cc-3' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled', has_changes: false },
    ]);
    const exchanges = getExchanges(map, id);
    expect(exchangeUserMessage(exchanges[0])).toBe('Continued the response');
  });

  it('ContinuationStarted from api actor with engine mode keeps the engine-restart label', () => {
    // `kind: 'api', mode: 'engine'` is the engine-driven REST path
    // (e.g. an internal tool calls Continue on behalf of the engine).
    // Same shape as the engine-restart recovery path — the wording must
    // not lie and claim a human clicked Continue.
    const { map, id } = makeThread();
    insertEvents(map, id, [
      {
        type: 'ContinuationStarted',
        branch: 'claude-code/20260318',
        actor: { kind: 'api', mode: 'engine', user_agent: 'engine-internal' },
      },
      { type: 'SessionStarted', session_id: 'cc-4' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled', has_changes: false },
    ]);
    const exchanges = getExchanges(map, id);
    expect(exchangeUserMessage(exchanges[0])).toBe('Resumed after engine restart');
  });

  it('ContinuationStarted in existing thread (with prior messages) creates new exchange', () => {
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
      { type: 'ContinuationStarted', branch: 'claude-code/20260318-122816' },
      { type: 'SessionStarted', session_id: 'cc-recovery', branch: 'claude-code/20260318-122816' },
      { type: 'CodingAgentTextStreamed', text: 'Continuing work.' },
      { type: 'ResponseGenerated' },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[1].userEvent.type).toBe('ContinuationStarted');
    expect(exchangeStatus(exchanges[1], '', true)).toBe('done');
  });

  it('ContinuationStarted that completes with SessionEnded shows Done', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'ContinuationStarted', branch: 'claude-code/20260318' },
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
      { type: 'ThoughtStreamed', text: 'Context: 5000 tokens, 3 messages', context_tokens: 5000, context_messages: 3 },
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
      { type: 'ThoughtStreamed', text: 'Context: 2000 tokens, 2 messages', context_tokens: 2000, context_messages: 2 },
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

  it('ToolResult content is attached to the matching step (LLM tools)', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'List files' },
      { type: 'ToolCalled', name: 'list_files', args: { path: '.' } },
      { type: 'ToolResult', name: 'list_files', result: 'a.txt\nb.txt' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = getExchanges(map, id);
    const stepEvents = exchangeResponseEvents(exchanges[0]).filter(e => e.type === 'step') as Array<{ description: string; result?: string }>;
    expect(stepEvents).toHaveLength(1);
    expect(stepEvents[0].result).toBe('a.txt\nb.txt');
  });

  it('CodingAgentToolResult content is paired by tool_use_id and attached to the matching step', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Run a command' },
      { type: 'CodingAgentUserMessageSent', text: 'Run a command' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'ls' }, tool_use_id: 'tu-1' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'pwd' }, tool_use_id: 'tu-2' },
      // Out-of-order results — pairing must follow tool_use_id, not arrival order
      { type: 'CodingAgentToolResult', name: 'Bash', result: '/home/user', tool_use_id: 'tu-2' },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'a.txt\nb.txt', tool_use_id: 'tu-1' },
      { type: 'CodingAgentIdled' },
    ]);

    const exchanges = getExchanges(map, id);
    const stepEvents = exchangeResponseEvents(exchanges[0]).filter(e => e.type === 'step') as Array<{ description: string; tool_use_id?: string; result?: string }>;
    const tu1 = stepEvents.find(s => s.tool_use_id === 'tu-1');
    const tu2 = stepEvents.find(s => s.tool_use_id === 'tu-2');
    expect(tu1?.result).toBe('a.txt\nb.txt');
    expect(tu2?.result).toBe('/home/user');
  });

  it('ContextAssembled attaches assembled-prompt context to subsequent steps in the exchange', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Search my notes' },
      {
        type: 'ContextAssembled',
        sections: [
          { name: 'System Instructions', content: 'You are a helpful assistant.', char_count: 28 },
          { name: 'User Message', content: 'Search my notes', char_count: 15 },
        ],
        tools: ['search_memory', 'read_file'],
        model: 'claude-opus-4-7',
        total_chars: 43,
      },
      { type: 'MemorySearched', results: 3, queries: ['notes'] },
      { type: 'ResponseGenerated' },
    ] as Array<ThreadEvent & { created?: string }>);

    const exchanges = getExchanges(map, id);
    const stepEvents = exchangeResponseEvents(exchanges[0]).filter(e => e.type === 'step') as Array<{ description: string; context?: { sections: Array<{ name: string }>; tools: string[]; model: string; total_chars: number } }>;
    expect(stepEvents).toHaveLength(1);
    expect(stepEvents[0].context).toBeDefined();
    expect(stepEvents[0].context?.sections.map(s => s.name)).toEqual(['System Instructions', 'User Message']);
    expect(stepEvents[0].context?.tools).toEqual(['search_memory', 'read_file']);
    expect(stepEvents[0].context?.model).toBe('claude-opus-4-7');
    expect(stepEvents[0].context?.total_chars).toBe(43);
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
// Post-restart reminder: only when the thread still needs the user
//
// A question-parked thread survives a restart with no abort (the engine's
// preserve guard). Answering it resumes the turn that asked — so the timeline
// must read exactly as it would have without the restart: no "Continued the
// response" boundary and no "Reminded the model …" engine note under a card the
// user already answered. A genuinely interrupted response is the opposite case:
// it still needs the user to click Continue, and that resume keeps its boundary
// AND its side-effect reminder.
// ---------------------------------------------------------------------------
describe('Flow: post-restart resume reminder', () => {
  it('question answered after restart → no boundary, no reminder, reply under the card', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'ask test user q', channel: 'chat', event_id: 'mr-1' },
      { type: 'ToolCalled', name: 'ask_user_question', args: {}, request_event_id: 'mr-1', event_id: 'call-1' },
      {
        type: 'UserQuestionAsked',
        tool_use_id: 'toolu-1#q0',
        question: 'Which test option would you like to choose?',
        options: [
          { id: 'opt-0', label: 'Option A' },
          { id: 'opt-1', label: 'Option B' },
        ],
      },
      // --- engine restart here: no abort is emitted, the card stays live ---
      { type: 'UserQuestionAnswered', tool_use_id: 'toolu-1#q0', answer: { kind: 'Selected', option_id: 'opt-0' } },
      {
        type: 'ToolResult',
        name: 'ask_user_question',
        result: '{"Which test option would you like to choose?":"Option A"}',
        tool_called_event_id: 'call-1',
        request_event_id: 'mr-1',
      },
      // The resumed turn continues the ORIGINAL request — no ContinuationStarted,
      // no engine-note UserPromptInjected.
      { type: 'TextStreamed', text: 'You selected **Option A**.', request_event_id: 'mr-1' },
      { type: 'ResponseGenerated', text: 'You selected **Option A**.', request_event_id: 'mr-1' },
    ] as Array<ThreadEvent & { created?: string; event_id?: string }>);

    const exchanges = getExchanges(map, id);
    expect(exchanges.map(e => e.userEvent.type)).not.toContain('ContinuationStarted');
    expect(exchanges.every(e => resumeEngineNote(e) === null)).toBe(true);

    // The reply groups under the question card, exactly as an uninterrupted
    // answer would (the divider owns its post-answer continuation).
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(divider).toBeDefined();
    expect(exchangeResponseText(divider)).toContain('Option A');
    expect(exchangeStatus(divider, '', true)).toBe('done');
  });

  it('genuinely interrupted response revived by Continue → boundary AND reminder', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'ping me', channel: 'chat', event_id: 'mr-2' },
      { type: 'ToolCalled', name: 'send_notification', args: {}, request_event_id: 'mr-2' },
      { type: 'ToolResult', name: 'send_notification', result: 'ok', request_event_id: 'mr-2' },
      { type: 'ResponseAborted', request_event_id: 'mr-2', cause: 'recovery_after_restart' },
      // The user clicked Continue: the resume opens its own boundary and the
      // engine note tells them what the model was told about the aborted run.
      { type: 'ContinuationStarted', actor: { kind: 'device', device_id: 'd-1', label: 'My iPhone' }, event_id: 'cs-1' },
      {
        type: 'UserPromptInjected',
        mode: 'engine',
        text: '[Engine note — this is a rerun]\n'
          + 'The interrupted run performed the following actions before the abort:\n'
          + '- send_notification(Ping) → ok',
        request_event_id: 'cs-1',
      },
      { type: 'TextStreamed', text: 'Already pinged you.', request_event_id: 'cs-1' },
      { type: 'ResponseGenerated', text: 'Already pinged you.', request_event_id: 'cs-1' },
    ] as Array<ThreadEvent & { created?: string; event_id?: string }>);

    const exchanges = getExchanges(map, id);
    const resume = exchanges.find(e => e.userEvent.type === 'ContinuationStarted')!;
    expect(resume).toBeDefined();
    expect(exchangeUserMessage(resume)).toBe('Continued the response');
    const note = resumeEngineNote(resume);
    expect(note).not.toBeNull();
    expect(note!.toolCount).toBe(1);
    expect(note!.text).toContain('send_notification');
  });
});

// ---------------------------------------------------------------------------
// CC thread follow-up: channel inheritance
// ---------------------------------------------------------------------------
