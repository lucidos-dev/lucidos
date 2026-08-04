import { describe, it, expect } from 'vitest';
import { groupIntoExchanges, exchangeResponseEvents, exchangeSteps, exchangeStatus, type StoredEvent, type ThreadEvent } from '../thread-events';

const ANSWER_FIRST = "You need light + dark: light for the macOS dock, dark for the PWA splash.";

function ev(seq: number, e: ThreadEvent, created = `2026-05-03T12:00:${String(seq).padStart(2, '0')}Z`) {
  return [seq, { ...e, created }] as const;
}
function thread(...entries: Array<readonly [number, StoredEvent]>) {
  return new Map(entries);
}

describe('groupIntoExchanges — ActionRequired events as exchange boundaries', () => {
  it('UserQuestionAsked starts a new exchange', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'hi' }),
      ev(2, { type: 'CodingAgentTextStreamed', text: 'thinking...' }),
      ev(3, { type: 'UserQuestionAsked', tool_use_id: 'tu1', cc_session_id: 's', question: 'X or Y?', options: [{ id: 'a', label: 'X' }] }),
      ev(4, { type: 'UserQuestionAnswered', tool_use_id: 'tu1', answer: { kind: 'Selected', option_id: 'a' } }),
      ev(5, { type: 'CodingAgentTextStreamed', text: 'continuing' }),
    );
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
    expect(exchanges[1].userEvent.type).toBe('UserQuestionAsked');
    const divSteps = exchanges[1].steps.map(s => s.event.type);
    expect(divSteps).toContain('UserQuestionAnswered');
    expect(divSteps).toContain('CodingAgentTextStreamed');
  });

  it('CodingAgentPermissionRequest starts a new exchange', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'edit foo' }),
      ev(2, { type: 'CodingAgentPermissionRequest', request_id: 'r1', tool_use_id: 'tu', tool_name: 'Edit', input: { file_path: '/foo' }, summary: 'Edit /foo' }),
      ev(3, { type: 'CodingAgentPermissionResolved', request_id: 'r1', allowed: true }),
    );
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('CodingAgentPermissionRequest');
  });

  it('chat agent PRE-question text lands above the question card and keeps the card answerable', () => {
    // The "answer first, then offer choices" turn: the agent streams its
    // explanation, THEN calls ask_user_question in the SAME turn. The engine
    // emits the explanation as TextStreamed BEFORE the ToolCalled /
    // UserQuestionAsked (lower sequence), so it groups into the MR exchange and
    // renders ABOVE the card. This is the rendering side of the engine streaming
    // the agent's explanation alongside the question instead of dropping it —
    // the root cause of "agent asks a question without explaining". The
    // pre-question text must NOT mark the divider overtaken (it precedes the
    // question, so the buttons stay live).
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'Do I need light and dark ones, and for what?', _eventId: 'msg-1', created: '2026-06-16T12:00:01Z' } as StoredEvent],
      [2, { type: 'ThoughtStreamed', text: 'Context: 1 tokens, 1 messages', request_event_id: 'msg-1', created: '2026-06-16T12:00:02Z' } as StoredEvent],
      [3, { type: 'TextStreamed', text: ANSWER_FIRST, request_event_id: 'msg-1', created: '2026-06-16T12:00:03Z' } as StoredEvent],
      [4, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-06-16T12:00:04Z' } as StoredEvent],
      [5, { type: 'UserQuestionAsked', tool_use_id: 'tu-outer-0', cc_session_id: '', question: 'So — which assets should I generate?', options: [{ id: 'a', label: 'Both' }], created: '2026-06-16T12:00:05Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['MessageReceived', 'UserQuestionAsked']);

    // The explanation renders in the MR exchange (above the card), not the divider.
    const mrEx = exchanges[0];
    expect(mrEx.steps.map(s => s.event.type)).toContain('TextStreamed');
    const mrResponse = exchangeResponseEvents(mrEx);
    expect(mrResponse.find(e => e.type === 'text' && e.md === ANSWER_FIRST)).toBeDefined();

    // The divider holds only the question; the pre-question text did not leak
    // into it, so the card is not marked overtaken and its buttons stay live.
    const divider = exchanges[1];
    expect(divider.steps.map(s => s.event.type)).not.toContain('TextStreamed');
    expect(divider.questionOvertaken).toBe(false);
  });

  it('chat agent post-answer text/response routes to the question exchange (rendered below the answer)', () => {
    // The agent's reply text must land in the question divider so it renders
    // BELOW the question card, not back in the MR exchange (which would put
    // the reply ABOVE the question). `ToolResult` for the question-asking
    // tool itself carries `tool_called_event_id` and is routed by the
    // sibling "Executing ask_user_question..." spinner test — it pairs with
    // its `ToolCalled` exchange (MR1), not the divider.
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'ask me a q', _eventId: 'msg-1', created: '2026-05-03T12:00:01Z' } as StoredEvent],
      [2, { type: 'ThoughtStreamed', text: 'thinking', request_event_id: 'msg-1', created: '2026-05-03T12:00:02Z' } as StoredEvent],
      [3, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-05-03T12:00:03Z' } as StoredEvent],
      [4, { type: 'UserQuestionAsked', tool_use_id: 'tu-outer-0', cc_session_id: '', question: 'evening?', options: [{ id: 'a', label: 'Code' }], created: '2026-05-03T12:00:04Z' } as StoredEvent],
      [5, { type: 'UserQuestionAnswered', tool_use_id: 'tu-outer-0', answer: { kind: 'Selected', option_id: 'a' }, created: '2026-05-03T12:00:05Z' } as StoredEvent],
      [6, { type: 'ToolResult', name: 'ask_user_question', result: 'ok', tool_called_event_id: 'tc-1', request_event_id: 'msg-1', created: '2026-05-03T12:00:06Z' } as StoredEvent],
      [7, { type: 'TextStreamed', text: "A coder's evening — fitting.", request_event_id: 'msg-1', created: '2026-05-03T12:00:07Z' } as StoredEvent],
      [8, { type: 'ResponseGenerated', text: "A coder's evening — fitting.", request_event_id: 'msg-1', created: '2026-05-03T12:00:08Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['MessageReceived', 'UserQuestionAsked']);

    const qSteps = exchanges[1].steps.map(s => s.event.type);
    expect(qSteps).toContain('UserQuestionAnswered');
    expect(qSteps).toContain('TextStreamed');
    expect(qSteps).toContain('ResponseGenerated');
    expect(qSteps).not.toContain('ToolResult');

    const mrSteps = exchanges[0].steps.map(s => s.event.type);
    expect(mrSteps).not.toContain('TextStreamed');
    expect(mrSteps).not.toContain('ResponseGenerated');
    expect(mrSteps).toContain('ToolResult');
  });

  it('live chat ToolResult resolves the "Executing ask_user_question..." spinner on the MR exchange', () => {
    // Regression: after the user answers, the agent loop emits ToolResult so
    // the next iteration can proceed. ToolCalled lives in the MR exchange;
    // UserQuestionAsked split off a divider, so the post-answer redirect
    // points at the divider. Without explicit pairing the live ToolResult
    // followed the redirect into the divider and never resolved the original
    // call's pending step — the MR exchange kept showing "↻ Executing
    // ask_user_question..." forever, even though the question card already
    // displayed the picked answer.
    //
    // Fix: the engine stamps `tool_called_event_id` on every live chat
    // ToolResult (not just synthetic recovery backfills), so the result
    // routes back to the ToolCalled's exchange via `chatToolCallOwners`.
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'ask me a q', _eventId: 'msg-1', created: '2026-05-03T12:00:01Z' } as StoredEvent],
      [2, { type: 'ThoughtStreamed', text: 'thinking', request_event_id: 'msg-1', created: '2026-05-03T12:00:02Z' } as StoredEvent],
      [3, { type: 'ToolCalled', name: 'ask_user_question', args: {}, description: 'Executing ask_user_question...', _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-05-03T12:00:03Z' } as StoredEvent],
      [4, { type: 'UserQuestionAsked', tool_use_id: 'tu-outer-0', cc_session_id: '', question: 'evening?', options: [{ id: 'a', label: 'Code' }], created: '2026-05-03T12:00:04Z' } as StoredEvent],
      [5, { type: 'UserQuestionAnswered', tool_use_id: 'tu-outer-0', answer: { kind: 'Selected', option_id: 'a' }, created: '2026-05-03T12:00:05Z' } as StoredEvent],
      [6, { type: 'ToolResult', name: 'ask_user_question', result: 'ok', tool_called_event_id: 'tc-1', request_event_id: 'msg-1', created: '2026-05-03T12:00:06Z' } as StoredEvent],
      [7, { type: 'ThoughtStreamed', text: 'next round', request_event_id: 'msg-1', created: '2026-05-03T12:00:07Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const mrEx = exchanges.find(e => e.userEvent.type === 'MessageReceived')!;
    // ToolResult lands with its ToolCalled, not in the question divider.
    expect(mrEx.steps.map(s => s.event.type)).toContain('ToolResult');
    const questionEx = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(questionEx.steps.map(s => s.event.type)).not.toContain('ToolResult');
    // The user-visible bug: the steps panel for the MR exchange.
    const steps = exchangeSteps(mrEx, /* _isLast */ false, /* threadIdle */ false);
    const askStep = steps.find(s => s.description === 'Executing ask_user_question...');
    expect(askStep).toBeDefined();
    expect(askStep!.outcome).toBe('success');
  });

  it('multiple pauses in one CC turn produce one divider per pause (MP1)', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'do stuff' }),
      ev(2, { type: 'CodingAgentPermissionRequest', request_id: 'r1', tool_use_id: 'tu1', tool_name: 'Bash', input: {}, summary: 'Bash ls' }),
      ev(3, { type: 'CodingAgentPermissionResolved', request_id: 'r1', allowed: true }),
      ev(4, { type: 'UserQuestionAsked', tool_use_id: 'tu2', cc_session_id: 's', question: 'choose', options: [] }),
      ev(5, { type: 'UserQuestionAnswered', tool_use_id: 'tu2', answer: { kind: 'FreeText', text: 'go' } }),
      ev(6, { type: 'CodingAgentPermissionRequest', request_id: 'r2', tool_use_id: 'tu3', tool_name: 'Edit', input: {}, summary: 'Edit x' }),
    );
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'CodingAgentPermissionRequest',
      'UserQuestionAsked',
      'CodingAgentPermissionRequest',
    ]);
  });
});

describe('groupIntoExchanges — question divider marked overtaken when agent progresses past it without an answer', () => {
  // CC's parallel-tool-call race: AskUserQuestion is emitted alongside
  // sibling tool_uses; the hook blocks the question while the siblings
  // dispatch and emit CodingAgent{TextStreamed,ToolCalled,ToolResult,…}
  // as steps of the divider before any answer lands. `questionOvertaken`
  // lets ChatExchange disable the QuestionCard buttons in that window.

  it('CC TextStreamed after UserQuestionAsked (no answer) marks divider as overtaken', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'do stuff' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu-q', cc_session_id: 's', question: 'X?', options: [{ id: 'a', label: 'X' }] }),
      ev(3, { type: 'CodingAgentTextStreamed', text: 'parallel work' }),
    );
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(divider.questionOvertaken).toBe(true);
  });

  it('CC ToolCalled after UserQuestionAsked (no answer) marks divider as overtaken', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'do stuff' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu-q', cc_session_id: 's', question: 'X?', options: [{ id: 'a', label: 'X' }] }),
      ev(3, { type: 'CodingAgentToolCalled', name: 'Bash', args: {}, tool_use_id: 'tu-sibling' }),
    );
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(divider.questionOvertaken).toBe(true);
  });

  it('chat TextStreamed after UserQuestionAsked marks divider as overtaken (chat symmetry)', () => {
    // Chat-agent path. Today this only happens if the agentic loop misbehaves
    // — the tool blocks sequentially — but the projection treats both agents
    // uniformly so a future regression can't reintroduce live buttons on a
    // question the agent has already raced past.
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'ask me', _eventId: 'msg-1', created: '2026-05-19T12:00:01Z' } as StoredEvent],
      [2, { type: 'UserQuestionAsked', tool_use_id: 'tu-q', cc_session_id: '', question: 'X?', options: [{ id: 'a', label: 'X' }], created: '2026-05-19T12:00:02Z' } as StoredEvent],
      [3, { type: 'TextStreamed', text: 'chat-agent kept talking', request_event_id: 'msg-1', created: '2026-05-19T12:00:03Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(divider.questionOvertaken).toBe(true);
  });

  it('matching UserQuestionAnswered beats progression — divider is NOT overtaken', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'do stuff' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu-q', cc_session_id: 's', question: 'X?', options: [{ id: 'a', label: 'X' }] }),
      ev(3, { type: 'CodingAgentTextStreamed', text: 'parallel work' }),
      ev(4, { type: 'UserQuestionAnswered', tool_use_id: 'tu-q', answer: { kind: 'Selected', option_id: 'a' } }),
    );
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(divider.questionOvertaken).toBe(false);
  });

  it('question is the last event in the thread — divider is NOT overtaken', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'ask' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu-q', cc_session_id: 's', question: 'X?', options: [{ id: 'a', label: 'X' }] }),
    );
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(divider.questionOvertaken).toBe(false);
  });

  it('answer for a DIFFERENT tool_use_id does not save an overtaken question', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'do stuff' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu-q', cc_session_id: 's', question: 'X?', options: [{ id: 'a', label: 'X' }] }),
      ev(3, { type: 'CodingAgentTextStreamed', text: 'parallel work' }),
      ev(4, { type: 'UserQuestionAnswered', tool_use_id: 'tu-OTHER', answer: { kind: 'Selected', option_id: 'a' } }),
    );
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(divider.questionOvertaken).toBe(true);
  });
});

describe('exchangeResponseEvents — divider exchange has no inline question/permission ResponseEvent', () => {
  it('UserQuestionAnswered + post-resume text in divider exchange omits inline question event', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'hi' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu1', cc_session_id: 's', question: 'q', options: [] }),
      ev(3, { type: 'UserQuestionAnswered', tool_use_id: 'tu1', answer: { kind: 'Selected', option_id: 'a' } }),
      ev(4, { type: 'CodingAgentTextStreamed', text: 'after' }),
    );
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges[1];
    const responseEvents = exchangeResponseEvents(divider);
    expect(responseEvents.find(e => e.type === 'question')).toBeUndefined();
    // The post-resume text is still in the response panel
    expect(responseEvents.find(e => e.type === 'text' && e.md === 'after')).toBeDefined();
  });
});

describe('exchangeStatus on divider exchanges', () => {
  it('pending question divider is awaiting-answer', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'hi' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu1', cc_session_id: 's', question: 'q', options: [] }),
    );
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges[1];
    expect(exchangeStatus(divider, '', true, false, true)).toBe('awaiting-answer');
  });

  it('answered question divider with CC resume is coding-agent-working', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'hi' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu1', cc_session_id: 's', question: 'q', options: [] }),
      ev(3, { type: 'UserQuestionAnswered', tool_use_id: 'tu1', answer: { kind: 'Selected', option_id: 'a' } }),
      ev(4, { type: 'CodingAgentTextStreamed', text: 'continuing' }),
    );
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges[1];
    expect(exchangeStatus(divider, '', true, false, true)).toBe('coding-agent-working');
  });

  it('canceled question divider settles into a non-awaiting terminal status', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'hi' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu1', cc_session_id: 's', question: 'q', options: [] }),
      ev(3, { type: 'UserQuestionAnswered', tool_use_id: 'tu1', answer: { kind: 'Canceled' } }),
    );
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges[1];
    // Status is no longer 'awaiting-answer' since the question is resolved.
    // Exact terminal value depends on existing logic; assert it's NOT awaiting.
    expect(exchangeStatus(divider, '', true, false, true)).not.toBe('awaiting-answer');
  });
});

describe('groupIntoExchanges — ResponseCanceled on a UserQuestion', () => {
  it('skips the boundary exchange when the question itself resolved as Canceled (the question card\'s cancel-as-picked button carries the signal)', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'hi' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu1', cc_session_id: 's', question: 'q', options: [] }),
      ev(3, { type: 'UserQuestionAnswered', tool_use_id: 'tu1', answer: { kind: 'Canceled' } }),
      ev(4, { type: 'ResponseCanceled' }),
    );
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['MessageReceived', 'UserQuestionAsked']);
    // The cancel itself still lives on the question exchange's steps so the
    // audit trail (and status='canceled' computation) stay intact.
    const div = exchanges[1];
    expect(div.steps.some(s => s.event.type === 'ResponseCanceled')).toBe(true);
  });

  it('keeps the boundary exchange when the user picked an option (Selected) before canceling — the picked option is shown, not a cancel button, so the You-panel is the only signal', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'hi' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu1', cc_session_id: 's', question: 'q', options: [{ id: 'a', label: 'A' }] }),
      ev(3, { type: 'UserQuestionAnswered', tool_use_id: 'tu1', answer: { kind: 'Selected', option_id: 'a' } }),
      ev(4, { type: 'CodingAgentTextStreamed', text: 'partial reply' }),
      ev(5, { type: 'ResponseCanceled' }),
    );
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'UserQuestionAsked',
      'ResponseCanceled',
    ]);
  });

  it('keeps the boundary exchange when the user picked MultiSelected before canceling', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'hi' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu1', cc_session_id: 's', question: 'q', options: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }] }),
      ev(3, { type: 'UserQuestionAnswered', tool_use_id: 'tu1', answer: { kind: 'MultiSelected', option_ids: ['a', 'b'] } }),
      ev(4, { type: 'ResponseCanceled' }),
    );
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'UserQuestionAsked',
      'ResponseCanceled',
    ]);
  });

  it('keeps the boundary exchange when the user typed FreeText before canceling', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'hi' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu1', cc_session_id: 's', question: 'q', options: [] }),
      ev(3, { type: 'UserQuestionAnswered', tool_use_id: 'tu1', answer: { kind: 'FreeText', text: 'go' } }),
      ev(4, { type: 'ResponseCanceled' }),
    );
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'UserQuestionAsked',
      'ResponseCanceled',
    ]);
  });

  it('keeps the boundary when ResponseCanceled lands with no UserQuestionAnswered yet — the question card has no resolution to render, so the You-panel is the only signal', () => {
    // Engine normally emits UserQuestionAnswered (kind: Canceled) ahead of
    // ResponseCanceled for an open question; this test pins the safe fallback
    // in case the ordering ever inverts or the engine drops the answer event.
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'hi' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu1', cc_session_id: 's', question: 'q', options: [] }),
      ev(3, { type: 'ResponseCanceled' }),
    );
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'UserQuestionAsked',
      'ResponseCanceled',
    ]);
  });

  it('still creates the separate boundary exchange when ResponseCanceled cancels a regular chat MessageReceived', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'hi' }),
      ev(2, { type: 'TextStreamed', text: 'partial' }),
      ev(3, { type: 'ResponseCanceled' }),
    );
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['MessageReceived', 'ResponseCanceled']);
  });

  it('still creates the separate boundary exchange when ResponseCanceled cancels a CodingAgentPermissionRequest divider', () => {
    // Permission requests own their own resolution UI (the picked button on
    // PermissionCard); a cancel that lands on a permission divider isn't the
    // user pressing Deny inside the card, so the You-panel attribution still
    // adds value. Keep the boundary exchange here.
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'do stuff' }),
      ev(2, { type: 'CodingAgentPermissionRequest', request_id: 'r1', tool_use_id: 'tu1', tool_name: 'Bash', input: {}, summary: 'Bash ls' }),
      ev(3, { type: 'ResponseCanceled' }),
    );
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'CodingAgentPermissionRequest',
      'ResponseCanceled',
    ]);
  });
});

describe('groupIntoExchanges — answer/resolution routing across an intervening boundary', () => {
  // Real thread 3e54cacb: a chat agent woken by a spawned CC sub-thread asks a
  // question. The sub-thread finishing emits ChildThreadCompleted — an exchange
  // boundary that becomes `current` BEFORE the answer lands. The answer
  // (FreeText, actor=thread_link) is NOT request-id routed, so it followed
  // `current` into the ChildThreadCompleted exchange instead of grouping with
  // its UserQuestionAsked divider. The divider then never saw its answer and
  // stayed stuck on 'awaiting-answer' forever even though the agent had resumed
  // and completed. Fix: route the answer to its divider by tool_use_id, the
  // same way CodingAgentToolResult is routed back to its call across a boundary.
  it('UserQuestionAnswered routes to its divider when a ChildThreadCompleted boundary intervened', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'check backups', _eventId: 'msg-1', created: '2026-05-30T04:21:00Z' } as StoredEvent],
      [2, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-05-30T04:21:01Z' } as StoredEvent],
      [3, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'How surfaced?', options: [{ id: 'a', label: 'Toast' }], created: '2026-05-30T04:21:02Z' } as StoredEvent],
      [4, { type: 'ChildThreadCompleted', child_thread_id: 'child-1', status: 'success', summary: 'engine fix landed', _eventId: 'ctc-1', created: '2026-05-30T04:26:15Z' } as StoredEvent],
      [5, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'FreeText', text: 'toast' }, created: '2026-05-30T04:26:15Z' } as StoredEvent],
      [6, { type: 'TextStreamed', text: 'Manual backup still grinding…', request_event_id: 'msg-1', created: '2026-05-30T04:26:35Z' } as StoredEvent],
      [7, { type: 'ResponseGenerated', text: 'Manual backup still grinding…', request_event_id: 'msg-1', created: '2026-05-30T04:26:36Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    const ctc = exchanges.find(e => e.userEvent.type === 'ChildThreadCompleted')!;

    // The answer groups with its divider, not the intervening boundary.
    expect(divider.steps.map(s => s.event.type)).toContain('UserQuestionAnswered');
    expect(ctc.steps.map(s => s.event.type)).not.toContain('UserQuestionAnswered');

    // With the answer present, the question is resolved — not overtaken, and the
    // status reflects the completed response instead of a stuck 'awaiting-answer'.
    expect(divider.questionOvertaken).toBe(false);
    expect(exchangeStatus(divider, '', false, false, false)).toBe('done');
  });

  it('CodingAgentPermissionResolved routes to its divider when a boundary intervened', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'edit foo', _eventId: 'msg-1', created: '2026-05-30T04:00:00Z' } as StoredEvent],
      [2, { type: 'CodingAgentPermissionRequest', request_id: 'r1', tool_use_id: 'tu', tool_name: 'Edit', input: {}, summary: 'Edit /foo', created: '2026-05-30T04:00:01Z' } as StoredEvent],
      [3, { type: 'ChildThreadCompleted', child_thread_id: 'c1', status: 'success', summary: 'x', _eventId: 'ctc-1', created: '2026-05-30T04:00:02Z' } as StoredEvent],
      [4, { type: 'CodingAgentPermissionResolved', request_id: 'r1', allowed: true, created: '2026-05-30T04:00:03Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges.find(e => e.userEvent.type === 'CodingAgentPermissionRequest')!;
    const ctc = exchanges.find(e => e.userEvent.type === 'ChildThreadCompleted')!;
    expect(divider.steps.map(s => s.event.type)).toContain('CodingAgentPermissionResolved');
    expect(ctc.steps.map(s => s.event.type)).not.toContain('CodingAgentPermissionResolved');
  });

  it('answer with no matching divider falls back to current-exchange routing', () => {
    // A UserQuestionAnswered whose tool_use_id matches no divider (legacy data,
    // or an answer that arrived before its Asked) must not vanish — fall through
    // to `current` so it still renders somewhere.
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'hi' }),
      ev(2, { type: 'CodingAgentTextStreamed', text: 'working' }),
      ev(3, { type: 'UserQuestionAnswered', tool_use_id: 'tu-orphan', answer: { kind: 'FreeText', text: 'go' } }),
    );
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].steps.map(s => s.event.type)).toContain('UserQuestionAnswered');
  });
});

describe('groupIntoExchanges — response continuation after a mid-flight ChildThreadCompleted', () => {
  // Real thread 4d193da8: the parent is mid-response when a spawned sub-thread
  // finishes. The engine injects the child summary into the running loop as a
  // WakeFromChild (no new request_event_id — the turn keeps the originating
  // MR's id) and emits ChildThreadCompleted as a timeline boundary. Every step
  // AFTER the boundary still carries the turn's req_id, so without moving the
  // redirect they group back into the pre-completion exchange — which sits
  // ABOVE the child-completion card. The user then sees the continued "Thinking
  // / Running Python" rendered BEFORE the card it logically follows. The
  // continuation must group UNDER the card (chronological order).

  it('post-completion steps group under the ChildThreadCompleted card, not the pre-completion exchange', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'make a gif', _eventId: 'msg-1', created: '2026-06-16T12:00:00Z' } as StoredEvent],
      [2, { type: 'ThoughtStreamed', text: 'thinking', request_event_id: 'msg-1', created: '2026-06-16T12:00:01Z' } as StoredEvent],
      [3, { type: 'ChildThreadCompleted', child_thread_id: 'child-1', child_thread_title: 'icons', status: 'success', summary: 'icons landed', _eventId: 'ctc-1', created: '2026-06-16T12:00:02Z' } as StoredEvent],
      [4, { type: 'ToolCalled', name: 'run_python', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-06-16T12:00:03Z' } as StoredEvent],
      [5, { type: 'TextStreamed', text: 'rendered', request_event_id: 'msg-1', created: '2026-06-16T12:00:04Z' } as StoredEvent],
      [6, { type: 'ResponseGenerated', text: 'rendered', request_event_id: 'msg-1', created: '2026-06-16T12:00:05Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['MessageReceived', 'ChildThreadCompleted']);

    const mr = exchanges[0];
    const ctc = exchanges[1];
    // Pre-completion thinking stays above the card.
    expect(mr.steps.map(s => s.event.type)).toContain('ThoughtStreamed');
    // The post-completion continuation groups under the card.
    const ctcTypes = ctc.steps.map(s => s.event.type);
    expect(ctcTypes).toContain('ToolCalled');
    expect(ctcTypes).toContain('TextStreamed');
    expect(ctcTypes).toContain('ResponseGenerated');
    // …and NOT back in the pre-completion exchange (which would render above).
    expect(mr.steps.map(s => s.event.type)).not.toContain('ResponseGenerated');
    expect(mr.steps.map(s => s.event.type)).not.toContain('TextStreamed');
  });

  it('continuation moves below the card even when an injected user message redirected the turn first', () => {
    // The exact 4d193da8 shape: a buffered second message is absorbed mid-flight
    // (setting reqIdRedirect → the injected MR exchange) BEFORE the child
    // completes. The redirect must then advance to the card so the post-
    // completion work lands under it, not in the injected-message exchange.
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'make a gif', _eventId: 'msg-1', created: '2026-06-16T12:00:00Z' } as StoredEvent],
      [2, { type: 'ThoughtStreamed', text: 'thinking', request_event_id: 'msg-1', created: '2026-06-16T12:00:01Z' } as StoredEvent],
      [3, { type: 'MessageReceived', text: 'or any way', _eventId: 'msg-2', created: '2026-06-16T12:00:02Z' } as StoredEvent],
      [4, { type: 'UserPromptInjected', text: 'or any way', mode: 'human', injected_message_id: 'msg-2', request_event_id: 'msg-1', created: '2026-06-16T12:00:03Z' } as StoredEvent],
      [5, { type: 'ThoughtStreamed', text: 'thinking more', request_event_id: 'msg-1', created: '2026-06-16T12:00:04Z' } as StoredEvent],
      [6, { type: 'ChildThreadCompleted', child_thread_id: 'child-1', child_thread_title: 'icons', status: 'success', summary: 'icons landed', _eventId: 'ctc-1', created: '2026-06-16T12:00:05Z' } as StoredEvent],
      [7, { type: 'ToolCalled', name: 'run_python', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-06-16T12:00:06Z' } as StoredEvent],
      [8, { type: 'ResponseGenerated', text: 'rendered', request_event_id: 'msg-1', created: '2026-06-16T12:00:07Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['MessageReceived', 'MessageReceived', 'ChildThreadCompleted']);

    const injected = exchanges[1]; // the "or any way" exchange
    const ctc = exchanges[2];
    // The injected-message exchange keeps only its pre-completion steps.
    expect(injected.steps.map(s => s.event.type)).not.toContain('ResponseGenerated');
    expect(injected.steps.map(s => s.event.type)).not.toContain('ToolCalled');
    // The continuation lands under the card.
    expect(ctc.steps.map(s => s.event.type)).toContain('ToolCalled');
    expect(ctc.steps.map(s => s.event.type)).toContain('ResponseGenerated');
  });

  it('does NOT move the continuation when the turn is paused at a question divider (3e54cacb is preserved)', () => {
    // Counterpart to the answer-routing test above: when the turn paused on a
    // question BEFORE the child completed, the post-answer reply belongs with
    // the question card (which itself sits above the card), not below an
    // unrelated child-completion card. The redirect must NOT advance to the
    // card in this case.
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'check backups', _eventId: 'msg-1', created: '2026-06-16T12:00:00Z' } as StoredEvent],
      [2, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-06-16T12:00:01Z' } as StoredEvent],
      [3, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'How surfaced?', options: [{ id: 'a', label: 'Toast' }], created: '2026-06-16T12:00:02Z' } as StoredEvent],
      [4, { type: 'ChildThreadCompleted', child_thread_id: 'child-1', status: 'success', summary: 'fix landed', _eventId: 'ctc-1', created: '2026-06-16T12:00:03Z' } as StoredEvent],
      [5, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'FreeText', text: 'toast' }, created: '2026-06-16T12:00:04Z' } as StoredEvent],
      [6, { type: 'TextStreamed', text: 'reply', request_event_id: 'msg-1', created: '2026-06-16T12:00:05Z' } as StoredEvent],
      [7, { type: 'ResponseGenerated', text: 'reply', request_event_id: 'msg-1', created: '2026-06-16T12:00:06Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    const ctc = exchanges.find(e => e.userEvent.type === 'ChildThreadCompleted')!;
    // The reply stays with the answered question, not the child-completion card.
    expect(divider.steps.map(s => s.event.type)).toContain('ResponseGenerated');
    expect(ctc.steps.map(s => s.event.type)).not.toContain('ResponseGenerated');
    expect(exchangeStatus(divider, '', false, false, false)).toBe('done');
  });

  it('re-anchors the answered divider BELOW the intervening card so the live reply renders last', () => {
    // Real thread 8144b43e: the agent asks a question; ten minutes later a
    // spawned sub-thread finishes and lands a ChildThreadCompleted card BELOW
    // the still-open question; the user then answers and the agent resumes.
    //
    // The redirect correctly stays on the divider (the test above), but the
    // divider was CREATED before the card, so it kept its earlier slot in the
    // timeline. Both user-visible halves follow from that one fact while the
    // thread is running:
    //   1. every post-answer step rendered ABOVE the child-completion card, and
    //   2. the stepless card — now the last exchange on a running thread —
    //      never received the continuation it fell through to 'pending' for,
    //      so the bottom of the thread sat frozen on "Requesting".
    // Together they read as a stuck agent while it was in fact working, just
    // higher up the page. Fix: re-anchor the divider to its resolution point.
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'do the release', _eventId: 'msg-1', created: '2026-07-28T13:50:05Z' } as StoredEvent],
      [2, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T13:56:39Z' } as StoredEvent],
      [3, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'Re-fold onto current main?', options: [{ id: 'opt-0', label: 'Re-fold' }], channel: 'chat', created: '2026-07-28T13:56:40Z' } as StoredEvent],
      // Sub-thread finishes while the card is on screen — a boundary lands
      // AFTER the divider was created.
      [4, { type: 'ChildThreadCompleted', child_thread_id: 'child-1', child_thread_title: 'Fixing Lucidos Installer Version Bug', status: 'success', summary: 'Hardening complete.', _eventId: 'ctc-1', created: '2026-07-28T14:06:50Z' } as StoredEvent],
      [5, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'opt-0' }, created: '2026-07-28T14:07:25Z' } as StoredEvent],
      // The question-asking tool's own result pairs back to its ToolCalled.
      [6, { type: 'ToolResult', name: 'ask_user_question', result: 'ok', tool_called_event_id: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T14:10:40Z' } as StoredEvent],
      // Live continuation — the thread is still RUNNING (no terminal yet).
      [7, { type: 'TextStreamed', text: 'Re-fold approved. Clearing the old worktree…', request_event_id: 'msg-1', created: '2026-07-28T14:10:40Z' } as StoredEvent],
      [8, { type: 'ToolCalled', name: 'run_bash', args: {}, _eventId: 'tc-2', request_event_id: 'msg-1', created: '2026-07-28T14:11:09Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);

    // The divider is re-anchored to the END — below the card that landed while
    // it waited — so its continuation is the last thing in the timeline.
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'ChildThreadCompleted',
      'UserQuestionAsked',
    ]);

    const ctc = exchanges[1];
    const divider = exchanges[2];

    // The reply still belongs to the card the user answered (3e54cacb intact).
    const divTypes = divider.steps.map(s => s.event.type);
    expect(divTypes).toContain('UserQuestionAnswered');
    expect(divTypes).toContain('TextStreamed');
    expect(divTypes).toContain('ToolCalled');
    expect(ctc.steps.map(s => s.event.type)).not.toContain('TextStreamed');

    // Half 2: the superseded card is terminal, not a phantom "Requesting"
    // spinner — it is no longer `isLast` on a running (non-idle) thread.
    expect(exchangeStatus(ctc, '', /*isLast*/ false, false, false, /*threadIdle*/ false, false)).toBe('done');
    // …and the live work reads as active on the exchange that actually has it.
    expect(exchangeStatus(divider, '', /*isLast*/ true, false, false, /*threadIdle*/ false, false)).toBe('streaming');
  });

  it('advances the redirect past an ALREADY-ANSWERED divider when a child completes mid-response', () => {
    // Same stuck-looking thread, reached from the other ordering: the question
    // is answered FIRST, the agent resumes, and only then does a spawned
    // sub-thread finish. The parked-divider exception must not apply here — the
    // turn is an ordinary in-flight response again, so this is the plain
    // 4d193da8 case and the redirect has to advance to the card. Gating that
    // exception on the divider's TYPE alone kept it in force forever after the
    // answer, so the post-completion work routed back up into the answered card
    // and the stepless card sat last on 'Requesting'.
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'go', _eventId: 'msg-1', created: '2026-07-28T10:00:00Z' } as StoredEvent],
      [2, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T10:00:01Z' } as StoredEvent],
      [3, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'q?', options: [{ id: 'a', label: 'A' }], created: '2026-07-28T10:00:02Z' } as StoredEvent],
      [4, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'a' }, created: '2026-07-28T10:00:03Z' } as StoredEvent],
      [5, { type: 'TextStreamed', text: 'spawning a sub-thread', request_event_id: 'msg-1', created: '2026-07-28T10:00:04Z' } as StoredEvent],
      [6, { type: 'ChildThreadCompleted', child_thread_id: 'c1', status: 'success', summary: 'done', _eventId: 'ctc-1', created: '2026-07-28T10:05:00Z' } as StoredEvent],
      [7, { type: 'ToolCalled', name: 'run_bash', args: {}, _eventId: 'tc-2', request_event_id: 'msg-1', created: '2026-07-28T10:05:01Z' } as StoredEvent],
      [8, { type: 'TextStreamed', text: 'continuing after the child', request_event_id: 'msg-1', created: '2026-07-28T10:05:02Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'UserQuestionAsked',
      'ChildThreadCompleted',
    ]);
    const divider = exchanges[1];
    const ctc = exchanges[2];

    // Pre-completion reply stays with the answered question.
    expect(divider.steps.map(s => s.event.type)).toContain('UserQuestionAnswered');
    expect(divider.steps.filter(s => s.event.type === 'TextStreamed')).toHaveLength(1);
    // The post-completion continuation groups UNDER the card, so the card is no
    // longer a stepless last exchange spinning 'Requesting' on a running thread.
    const ctcTypes = ctc.steps.map(s => s.event.type);
    expect(ctcTypes).toContain('ToolCalled');
    expect(ctcTypes).toContain('TextStreamed');
    expect(exchangeStatus(ctc, '', /*isLast*/ true, false, false, /*threadIdle*/ false, false)).not.toBe('pending');
  });

  it('leaves a CC permission divider in place — its continuation flows to the intervening card', () => {
    // Counterpart gate: `CodingAgentPermissionRequest` is never a reqIdRedirect
    // target (CC events aren't request-id routed), so CC's post-grant work
    // follows `current` into the intervening boundary. Moving the divider below
    // that boundary would strand the card under its own continuation.
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'edit foo', _eventId: 'msg-1', created: '2026-07-28T04:00:00Z' } as StoredEvent],
      [2, { type: 'CodingAgentPermissionRequest', request_id: 'r1', tool_use_id: 'tu', tool_name: 'Edit', input: {}, summary: 'Edit /foo', created: '2026-07-28T04:00:01Z' } as StoredEvent],
      [3, { type: 'ChildThreadCompleted', child_thread_id: 'c1', status: 'success', summary: 'x', _eventId: 'ctc-1', created: '2026-07-28T04:00:02Z' } as StoredEvent],
      [4, { type: 'CodingAgentPermissionResolved', request_id: 'r1', allowed: true, created: '2026-07-28T04:00:03Z' } as StoredEvent],
      [5, { type: 'CodingAgentTextStreamed', text: 'edited', created: '2026-07-28T04:00:04Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'CodingAgentPermissionRequest',
      'ChildThreadCompleted',
    ]);
    // The grant routes back to its divider; the CC continuation stays with the
    // boundary that is `current`.
    expect(exchanges[1].steps.map(s => s.event.type)).toContain('CodingAgentPermissionResolved');
    expect(exchanges[2].steps.map(s => s.event.type)).toContain('CodingAgentTextStreamed');
  });
});

describe('continuationMoved — stale Thinking marker after a mid-flight handoff', () => {
  // Real thread 8144b43e: the agent was mid-thought when a spawned sub-thread
  // finished. The wake lands as a `ChildThreadCompleted` boundary and the turn's
  // continuation moves to that card — so the `ThoughtStreamed` marker left
  // pending in the pre-completion exchange has nothing left that can resolve it.
  // The turn is still running (no terminal, thread not idle), so neither
  // finalize trigger fired and an old "Thinking" row kept shimmering half a
  // screen above the exchange the agent was actually working in.
  const HANDOFF = new Map<number, StoredEvent>([
    [1, { type: 'MessageReceived', text: 'do the release', _eventId: 'msg-1', created: '2026-07-28T20:14:00Z' } as StoredEvent],
    [2, { type: 'ThoughtStreamed', text: 'planning', request_event_id: 'msg-1', created: '2026-07-28T20:14:01Z' } as StoredEvent],
    [3, { type: 'ToolCalled', name: 'run_bash', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T20:14:02Z' } as StoredEvent],
    [4, { type: 'ToolResult', name: 'run_bash', result: 'ok', tool_called_event_id: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T20:14:03Z' } as StoredEvent],
    // The marker that strands: the LLM was re-invoked and the child completed
    // before it produced any output.
    [5, { type: 'ThoughtStreamed', text: 'thinking again', request_event_id: 'msg-1', created: '2026-07-28T20:14:04Z' } as StoredEvent],
    [6, { type: 'ChildThreadCompleted', child_thread_id: 'child-1', child_thread_title: 'Fixing Duplicate Apple Notarization Submissions', status: 'canceled', summary: 'canceled', _eventId: 'ctc-1', created: '2026-07-28T20:15:03Z' } as StoredEvent],
    // The turn keeps streaming under the card — still running, no terminal.
    [7, { type: 'ToolCalled', name: 'run_bash', args: {}, _eventId: 'tc-2', request_event_id: 'msg-1', created: '2026-07-28T20:15:50Z' } as StoredEvent],
    [8, { type: 'ToolResult', name: 'run_bash', result: 'ok', tool_called_event_id: 'tc-2', request_event_id: 'msg-1', created: '2026-07-28T20:15:51Z' } as StoredEvent],
    [9, { type: 'ThoughtStreamed', text: 'still going', request_event_id: 'msg-1', created: '2026-07-28T20:15:52Z' } as StoredEvent],
  ]);

  it('drops the orphaned Thinking marker from the handed-off exchange while the turn runs on', () => {
    const exchanges = groupIntoExchanges(HANDOFF);
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['MessageReceived', 'ChildThreadCompleted']);
    const mr = exchanges[0];
    expect(mr.continuationMoved).toBe(true);

    // Thread is RUNNING (not idle) and the exchange has no terminal event.
    const events = exchangeResponseEvents(mr, /* isLast */ false, /* threadIdle */ false);
    expect(events.filter(e => e.type === 'step' && e.outcome === 'pending')).toHaveLength(0);
    // The bare trailing marker is noise once it can never resolve — same
    // treatment a completed exchange gets.
    const last = events[events.length - 1];
    expect(last.type === 'step' && last.description === 'Thinking').toBe(false);
    // The summary step list agrees with the inline one.
    expect(exchangeSteps(mr, /* isLast */ false, /* threadIdle */ false).filter(s => s.outcome === 'pending')).toHaveLength(0);
  });

  it('leaves the live Thinking marker alone in the exchange that took the continuation', () => {
    const exchanges = groupIntoExchanges(HANDOFF);
    const ctc = exchanges[1];
    expect(ctc.continuationMoved).toBeFalsy();
    const events = exchangeResponseEvents(ctc, /* isLast */ true, /* threadIdle */ false);
    expect(events.filter(e => e.type === 'step' && e.outcome === 'pending')).toHaveLength(1);
    expect(exchangeStatus(ctc, '', /* isLast */ true, false, false, /* threadIdle */ false, false)).toBe('streaming');
  });

  it('keeps a pending TOOL step spinning — its result can still re-route back by tool id', () => {
    // The `ask_user_question` shape: the call's "Executing …" spinner belongs to
    // the MR exchange and is resolved by a ToolResult that routes back by
    // `tool_called_event_id` long after the divider took the continuation. Only
    // Thinking markers are stale on a handoff, never tool steps.
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'do it', _eventId: 'msg-1', created: '2026-07-28T09:00:00Z' } as StoredEvent],
      [2, { type: 'ToolCalled', name: 'ask_user_question', args: {}, description: 'Executing ask_user_question...', _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T09:00:01Z' } as StoredEvent],
      [3, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'q?', options: [{ id: 'a', label: 'A' }], created: '2026-07-28T09:00:02Z' } as StoredEvent],
    ]);
    const mr = groupIntoExchanges(events)[0];
    expect(mr.continuationMoved).toBe(true);
    const rendered = exchangeResponseEvents(mr, /* isLast */ false, /* threadIdle */ false);
    const pending = rendered.filter(e => e.type === 'step' && e.outcome === 'pending');
    expect(pending).toHaveLength(1);
    expect(pending[0].type === 'step' && pending[0].description).toBe('Executing ask_user_question...');
  });

  it('does NOT mark a turn whose follow-up is still queued — the loop is still working in it', () => {
    // Mid-flight injection: the user's follow-up lands as a stepless
    // MessageReceived while the agent keeps streaming under the FIRST message's
    // request id. The first exchange is non-last but very much alive, so its
    // Thinking marker must keep shimmering.
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'first', _eventId: 'msg-1', created: '2026-07-28T09:10:00Z' } as StoredEvent],
      [2, { type: 'ToolCalled', name: 'run_bash', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T09:10:01Z' } as StoredEvent],
      [3, { type: 'ToolResult', name: 'run_bash', result: 'ok', tool_called_event_id: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T09:10:02Z' } as StoredEvent],
      [4, { type: 'MessageReceived', text: 'second', _eventId: 'msg-2', created: '2026-07-28T09:10:03Z' } as StoredEvent],
      [5, { type: 'ThoughtStreamed', text: 'still on the first', request_event_id: 'msg-1', created: '2026-07-28T09:10:04Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const first = exchanges[0];
    expect(first.continuationMoved).toBeFalsy();
    const rendered = exchangeResponseEvents(first, /* isLast */ false, /* threadIdle */ false);
    const pending = rendered.filter(e => e.type === 'step' && e.outcome === 'pending');
    expect(pending).toHaveLength(1);
    expect(pending[0].type === 'step' && pending[0].description).toBe('Thinking');
  });

  it('marks the exchange that actually owned the turn, not just the queued message in front of it', () => {
    // The 194474de shape: an uningested queued follow-up is `current` when the
    // divider is raised, so the exchange the redirect is moved OFF is the older
    // one that owns the turn's req_id — that is the one whose markers strand.
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'first', _eventId: 'msg-1', created: '2026-07-28T09:30:00Z' } as StoredEvent],
      [2, { type: 'ThoughtStreamed', text: 'planning', request_event_id: 'msg-1', created: '2026-07-28T09:30:01Z' } as StoredEvent],
      // Queued follow-up lands, then the turn raises a question.
      [3, { type: 'MessageReceived', text: 'second', _eventId: 'msg-2', created: '2026-07-28T09:30:02Z' } as StoredEvent],
      [4, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'q?', options: [{ id: 'a', label: 'A' }], created: '2026-07-28T09:30:03Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const first = exchanges.find(e => e.userEvent._eventId === 'msg-1')!;
    expect(first.continuationMoved).toBe(true);
    const rendered = exchangeResponseEvents(first, /* isLast */ false, /* threadIdle */ false);
    expect(rendered.filter(e => e.type === 'step' && e.outcome === 'pending')).toHaveLength(0);
  });

  it('clears the mark when the handed-off exchange takes the continuation back', () => {
    // A queued follow-up can be `previousCurrent` when a divider is raised (real
    // thread 194474de), so it gets marked without ever having owned the turn.
    // When its `UserPromptInjected` is absorbed it becomes the active turn — the
    // mark must not survive and freeze its live spinner.
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'first', _eventId: 'msg-1', created: '2026-07-28T09:20:00Z' } as StoredEvent],
      [2, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T09:20:01Z' } as StoredEvent],
      [3, { type: 'MessageReceived', text: 'second', _eventId: 'msg-2', created: '2026-07-28T09:20:02Z' } as StoredEvent],
      [4, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'q?', options: [{ id: 'a', label: 'A' }], created: '2026-07-28T09:20:03Z' } as StoredEvent],
      [5, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'a' }, created: '2026-07-28T09:20:04Z' } as StoredEvent],
      [6, { type: 'ToolResult', name: 'ask_user_question', result: 'a', tool_called_event_id: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T09:20:05Z' } as StoredEvent],
      // The queued follow-up is ingested — it now owns the turn.
      [7, { type: 'UserPromptInjected', text: 'second', mode: 'human', injected_message_id: 'msg-2', request_event_id: 'msg-1', created: '2026-07-28T09:20:06Z' } as StoredEvent],
      [8, { type: 'ThoughtStreamed', text: 'on the follow-up now', request_event_id: 'msg-1', created: '2026-07-28T09:20:07Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const followup = exchanges.find(e => e.userEvent._eventId === 'msg-2')!;
    expect(followup.continuationMoved).toBeFalsy();
    const rendered = exchangeResponseEvents(followup, /* isLast */ true, /* threadIdle */ false);
    expect(rendered.filter(e => e.type === 'step' && e.outcome === 'pending')).toHaveLength(1);
  });
});

describe('groupIntoExchanges — CodingAgentToolResult routing across permission boundaries', () => {
  it('routes tool result back to its originating exchange when a permission request split them', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'create a PR' }),
      ev(2, { type: 'CodingAgentTextStreamed', text: 'Push succeeded. Now opening the PR…' }),
      ev(3, { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'gh pr create' }, tool_use_id: 'toolu_pr' }),
      ev(4, { type: 'CodingAgentPermissionRequest', request_id: 'r1', tool_use_id: 'toolu_pr', tool_name: 'Bash', input: { command: 'gh pr create' }, summary: 'gh pr create' }),
      ev(5, { type: 'CodingAgentPermissionResolved', request_id: 'r1', allowed: true }),
      ev(6, { type: 'CodingAgentToolResult', name: '', result: 'PR created', tool_use_id: 'toolu_pr' }),
      ev(7, { type: 'CodingAgentTextStreamed', text: 'PR opened — updating my todo.' }),
      ev(8, { type: 'CodingAgentToolCalled', name: 'TodoWrite', args: {}, tool_use_id: 'toolu_todo' }),
      ev(9, { type: 'CodingAgentToolResult', name: '', result: 'ok', tool_use_id: 'toolu_todo' }),
      ev(10, { type: 'ResponseGenerated' }),
      ev(11, { type: 'CodingAgentIdled' }),
    );
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);

    const exA = exchanges[0];
    expect(exA.userEvent.type).toBe('MessageReceived');
    const aTypes = exA.steps.map(s => s.event.type);
    expect(aTypes).toContain('CodingAgentToolCalled');
    expect(aTypes).toContain('CodingAgentToolResult');
    const aSteps = exchangeResponseEvents(exA, false, false).filter(e => e.type === 'step');
    const ghStep = aSteps.find(s => s.tool_name === 'Bash');
    expect(ghStep?.outcome).toBe('success');

    const exB = exchanges[1];
    expect(exB.userEvent.type).toBe('CodingAgentPermissionRequest');
    const bResults = exB.steps.filter(s => s.event.type === 'CodingAgentToolResult');
    expect(bResults).toHaveLength(1);
    expect((bResults[0].event as { tool_use_id?: string }).tool_use_id).toBe('toolu_todo');
  });

  it('legacy events without tool_use_id fall back to current-exchange routing', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'hi' }),
      ev(2, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
      ev(3, { type: 'CodingAgentToolResult', name: '', result: 'ok' }),
      ev(4, { type: 'ResponseGenerated' }),
      ev(5, { type: 'CodingAgentIdled' }),
    );
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);
    const steps = exchangeResponseEvents(exchanges[0]).filter(e => e.type === 'step');
    const step = steps.find(s => s.tool_name === 'Bash');
    expect(step?.outcome).toBe('success');
  });
});

describe('groupIntoExchanges — synthetic ToolResult routing via tool_called_event_id', () => {
  // Regression: when the engine restarts mid-`ask_user_question`, the chat
  // agent has emitted ToolCalled+UserQuestionAsked but no ToolResult. The
  // post-restart recovery sweep emits a synthetic ToolResult with
  // `tool_called_event_id` pointing back at the orphan ToolCalled. Without
  // routing on that id, the synthetic result flows via `request_event_id`
  // through the UserQuestionAsked redirect into the ResponseAborted boundary
  // exchange — leaving the ToolCalled's "Executing ask_user_question..."
  // spinner spinning forever in its original (CTC/MR) exchange.
  it('synthetic ToolResult lands in the same exchange as its ToolCalled', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'do it', _eventId: 'msg-1', created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [2, { type: 'TextStreamed', text: 'Want me to:', request_event_id: 'msg-1', created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [3, { type: 'ToolCalled', name: 'ask_user_question', args: {}, description: 'Executing ask_user_question...', _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [4, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'X?', options: [{ id: 'a', label: 'X' }], created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [5, { type: 'ResponseAborted', text: 'interrupted by engine restart', cause: 'engine_shutdown', request_event_id: 'msg-1', created: '2026-05-22T06:52:18Z' } as StoredEvent],
      // Synthetic emit from `recover_orphan_tool_calls` — carries
      // tool_called_event_id pointing back at the orphan ToolCalled but NO
      // request_event_id (the recovery emit doesn't set one).
      [6, { type: 'ToolResult', name: 'ask_user_question', result: '[Tool execution interrupted by engine restart — original ToolCalled event_id: tc-1]', tool_called_event_id: 'tc-1', created: '2026-05-22T06:53:49Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    // Three exchanges: MR (with TextStreamed + ToolCalled + synthetic ToolResult),
    // UserQuestionAsked (with ResponseAborted via dual-push), ResponseAborted boundary.
    const mrEx = exchanges.find(e => e.userEvent.type === 'MessageReceived')!;
    const mrSteps = mrEx.steps.map(s => s.event.type);
    expect(mrSteps).toContain('ToolCalled');
    expect(mrSteps).toContain('ToolResult');

    // The synthetic ToolResult must NOT land in the UserQuestionAsked or
    // ResponseAborted boundary exchanges — that would strand the original
    // ToolCalled's spinner.
    const questionEx = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(questionEx.steps.map(s => s.event.type)).not.toContain('ToolResult');

    // Pairing succeeds: the InlineStep for the ToolCalled resolves (its
    // outcome leaves 'pending' once the ToolResult lands in the same exchange).
    const mrResponseEvents = exchangeResponseEvents(mrEx);
    const askStep = mrResponseEvents.find(e => e.type === 'step' && e.tool_name === 'ask_user_question');
    expect(askStep).toBeDefined();
    expect(askStep!.type === 'step' && askStep!.outcome).toBe('success');
  });

  it('synthetic ToolResult without tool_called_event_id falls back to current-exchange routing', () => {
    // Backward compat: legacy synthetic ToolResults (pre-tool_called_event_id)
    // still fall through to current — the wire field is optional, so
    // un-stamped emits must keep the old behavior.
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'do it', _eventId: 'msg-1', created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [2, { type: 'ToolCalled', name: 'run_python', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [3, { type: 'ToolResult', name: 'run_python', result: 'ok', request_event_id: 'msg-1', created: '2026-05-22T06:51:41Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(1);
    const steps = exchanges[0].steps.map(s => s.event.type);
    expect(steps).toContain('ToolResult');
  });
});

describe('groupIntoExchanges — question after a ChildThreadCompleted whose parent turn already finished', () => {
  // Real thread 2e98b44a ("Toasts gömda bakom tangentbordet"): the first turn
  // completes (ResponseGenerated under the MR's id). LATER a spawned sub-thread
  // is canceled → ChildThreadCompleted lands as a boundary and the engine starts
  // a FRESH turn anchored on the completion card's OWN id (req_id === ctc id, not
  // the old MR id). That turn asks a question; the user answers (FreeText), and
  // the agent's reply completes.
  //
  // Bug: when the card was created, the redirect bootstrap set a SPURIOUS
  // `oldMR.id → card` entry (the card's continuation actually routes by the
  // card's own id, so that entry is dead). When the question divider was then
  // created, its redirect loop found+moved that spurious entry, set
  // `updatedExisting`, and SKIPPED bootstrapping `card.id → divider`. The
  // post-answer reply (req_id === card id) therefore misrouted back INTO the
  // card — rendered ABOVE the question — and the divider, left with only
  // UserQuestionAnswered and no terminal, flashed 'aborted'.
  it('post-answer reply routes to the question divider, not the child card; divider is done, not aborted', () => {
    const events = new Map<number, StoredEvent>([
      // First turn: a normal MR that completes on its own.
      [1, { type: 'MessageReceived', text: 'toasts under keyboard', _eventId: 'msg-1', created: '2026-06-21T23:34:58Z' } as StoredEvent],
      [2, { type: 'TextStreamed', text: 'looking into it', request_event_id: 'msg-1', created: '2026-06-21T23:36:02Z' } as StoredEvent],
      [3, { type: 'ResponseGenerated', text: 'looking into it', request_event_id: 'msg-1', created: '2026-06-21T23:36:08Z' } as StoredEvent],
      // A spawned sub-thread finishes AFTER the turn completed → fresh turn
      // anchored on the card's own id (ctc-1).
      [4, { type: 'ChildThreadCompleted', child_thread_id: 'child-1', child_thread_title: 'Fixing iOS Keyboard Toast Occlusion', status: 'canceled', summary: '', _eventId: 'ctc-1', created: '2026-06-21T23:36:18Z' } as StoredEvent],
      [5, { type: 'ThoughtStreamed', text: 'thinking', request_event_id: 'ctc-1', created: '2026-06-21T23:36:19Z' } as StoredEvent],
      [6, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'ctc-1', created: '2026-06-21T23:36:27Z' } as StoredEvent],
      [7, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'proceed?', options: [{ id: 'opt-0', label: 'Restart the agent' }], created: '2026-06-21T23:36:27Z' } as StoredEvent],
      [8, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'FreeText', text: 'Nei i handlelisye appen' }, created: '2026-06-21T23:36:32Z' } as StoredEvent],
      // The question-asking tool's own result pairs back to its ToolCalled
      // (stays above the card with the spinner), NOT into the divider.
      [9, { type: 'ToolResult', name: 'ask_user_question', result: 'ok', tool_called_event_id: 'tc-1', request_event_id: 'ctc-1', created: '2026-06-21T23:36:32Z' } as StoredEvent],
      // The genuine post-answer continuation — must land in the divider (below
      // the answer), and its terminal must settle the divider to 'done'.
      [10, { type: 'TextStreamed', text: 'Fixed — it was the Handleliste app.', request_event_id: 'ctc-1', created: '2026-06-21T23:36:40Z' } as StoredEvent],
      [11, { type: 'ResponseGenerated', text: 'Fixed — it was the Handleliste app.', request_event_id: 'ctc-1', created: '2026-06-21T23:37:16Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'ChildThreadCompleted',
      'UserQuestionAsked',
    ]);

    const ctc = exchanges.find(e => e.userEvent.type === 'ChildThreadCompleted')!;
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;

    // The reply groups UNDER the question card (rendered below the answer),
    // not back in the child-completion card (which sits above the question).
    const divTypes = divider.steps.map(s => s.event.type);
    expect(divTypes).toContain('UserQuestionAnswered');
    expect(divTypes).toContain('TextStreamed');
    expect(divTypes).toContain('ResponseGenerated');

    const ctcTypes = ctc.steps.map(s => s.event.type);
    expect(ctcTypes).not.toContain('TextStreamed');
    expect(ctcTypes).not.toContain('ResponseGenerated');

    // The divider settles to 'done' on its terminal — never the stale 'aborted'
    // that the screenshot showed. Worst-case client flags: thread reports idle
    // (status NOT waiting_for_user_answer) at the last exchange.
    expect(exchangeStatus(divider, '', /*isLast*/ true, false, false, /*threadIdle*/ true, /*threadAwaitingAnswer*/ false)).toBe('done');
  });
});

describe('groupIntoExchanges — question asked while a queued follow-up message is pending', () => {
  // Real thread 194474de ("Trenger jeg underlag til bassenget"): the user sends
  // a message ("Trenger vi håv?", msg-1) and, while that turn streams, types a
  // second follow-up ("Til hva?", msg-2) which QUEUES — it becomes the `current`
  // exchange but is never ingested (no UserPromptInjected). The first turn then
  // calls ask_user_question (req_id = msg-1) and parks on the divider. The user
  // answers; the agent resumes and completes — every continuation event still
  // carries the turn's req_id (msg-1).
  //
  // Bug: when the divider was created, `previousCurrent` was the QUEUED msg-2
  // exchange (not the turn's msg-1 exchange), so the redirect bootstrap anchored
  // on msg-2's id instead of the turn's real req_id. The post-answer
  // TextStreamed / ResponseGenerated (req msg-1) therefore routed back to the
  // ORIGINAL msg-1 exchange (above the card), leaving the divider with only the
  // answer and no terminal — which the `threadIdle && !awaiting && hasSteps`
  // stale-detector renders as a PERSISTENT "Aborted" once the thread idles
  // (deterministic from history, so it survives reloads — not just a flash).
  //
  // Fix: track the turn's real req_id (`lastChatTurnReqId`, set by the
  // divider-raising tool call) and redirect THAT to the divider, independent of
  // whatever queued exchange happens to be `current`.
  it('post-answer reply routes to the divider, not the original message exchange; divider is done, not aborted', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'Trenger vi håv?', _eventId: 'msg-1', created: '2026-06-22T14:50:00Z' } as StoredEvent],
      [2, { type: 'ThoughtStreamed', text: 'thinking', request_event_id: 'msg-1', created: '2026-06-22T14:50:01Z' } as StoredEvent],
      // Queued follow-up typed while the turn streamed — becomes `current`, never ingested.
      [3, { type: 'MessageReceived', text: 'Til hva?', _eventId: 'msg-2', created: '2026-06-22T14:50:05Z' } as StoredEvent],
      [4, { type: 'TextStreamed', text: 'Ja, en håv er verdt det.', request_event_id: 'msg-1', created: '2026-06-22T14:50:26Z' } as StoredEvent],
      [5, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-06-22T14:50:26Z' } as StoredEvent],
      [6, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'Skal jeg legge til på handlelista?', options: [{ id: 'opt-0', label: 'Håv' }], multi_select: true, channel: 'chat', created: '2026-06-22T14:50:26Z' } as StoredEvent],
      // User removes the queued follow-up, then answers with custom text only.
      [7, { type: 'QueuedMessageRemoved', removed_message_id: 'msg-2', created: '2026-06-22T15:07:44Z' } as StoredEvent],
      [8, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'MultiSelected', option_ids: [], text: 'Kan jeg bruke presenning som underlag?' }, created: '2026-06-22T15:08:15Z' } as StoredEvent],
      // The question-asking tool's own result pairs back to its ToolCalled (msg-1 exchange).
      [9, { type: 'ToolResult', name: 'ask_user_question', result: 'ok', tool_called_event_id: 'tc-1', request_event_id: 'msg-1', created: '2026-06-22T15:08:15Z' } as StoredEvent],
      // The genuine post-answer continuation — must land in the divider (below the answer).
      [10, { type: 'ThoughtStreamed', text: 'thinking', request_event_id: 'msg-1', created: '2026-06-22T15:08:33Z' } as StoredEvent],
      [11, { type: 'TextStreamed', text: 'Ja, en presenning fungerer fint.', request_event_id: 'msg-1', created: '2026-06-22T15:08:33Z' } as StoredEvent],
      [12, { type: 'ResponseGenerated', text: 'Ja, en presenning fungerer fint.', request_event_id: 'msg-1', created: '2026-06-22T15:08:33Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    const mr1 = exchanges.find(e => e.userEvent.type === 'MessageReceived' && e.userEvent._eventId === 'msg-1')!;

    // The reply groups UNDER the question card (rendered below the answer),
    // not back in the original message exchange (which sits above the question).
    const divTypes = divider.steps.map(s => s.event.type);
    expect(divTypes).toContain('UserQuestionAnswered');
    expect(divTypes).toContain('TextStreamed');
    expect(divTypes).toContain('ResponseGenerated');

    // The original message exchange keeps its own pre-question work + the
    // ask_user_question ToolResult, but NOT the post-answer reply/terminal.
    const mr1Types = mr1.steps.map(s => s.event.type);
    expect(mr1Types).toContain('ToolCalled');
    expect(mr1Types).toContain('ToolResult');
    expect(mr1Types).not.toContain('ResponseGenerated');

    // The divider settles to 'done' on its terminal — never the persistent
    // 'aborted' the user reported. Worst-case client flags: thread reports idle
    // (status NOT waiting_for_user_answer) at the last exchange.
    expect(exchangeStatus(divider, '', /*isLast*/ true, false, false, /*threadIdle*/ true, /*threadAwaitingAnswer*/ false)).toBe('done');
  });
});

describe('exchangeStatus + responseTerminated — UserQuestionAsked exchange aborted by engine restart', () => {
  // The user-facing bug: a chat agent woken from ChildThreadCompleted asks a
  // question, the engine restarts, recovery emits ResponseAborted carrying
  // the ORIGINATING event's id (the CTC, not an older MR). The abort routes
  // via reqIdRedirect into the UserQuestionAsked exchange. Status should be
  // 'aborted' so the QuestionBody renders terminated (disabled buttons).
  it('ResponseAborted with the originating req_id terminates the UserQuestionAsked exchange', () => {
    const events = new Map<number, StoredEvent>([
      // ChildThreadCompleted is the originating event of the in-flight turn.
      [1, { type: 'ChildThreadCompleted', child_thread_id: 'child-1', child_thread_title: 'child', status: 'success', summary: 'done', _eventId: 'ctc-1', created: '2026-05-22T06:51:27Z' } as StoredEvent],
      [2, { type: 'TextStreamed', text: 'Want me to:', request_event_id: 'ctc-1', created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [3, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'ctc-1', created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [4, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'Both pending Apply. Next?', options: [{ id: 'a', label: 'Apply both' }], created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [5, { type: 'ResponseAborted', text: 'interrupted', cause: 'engine_shutdown', request_event_id: 'ctc-1', channel: 'chat', created: '2026-05-22T06:52:18Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const questionEx = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;

    // The dual-push pushes ResponseAborted into the question exchange's
    // steps (via the redirect set when the question was processed).
    expect(questionEx.steps.map(s => s.event.type)).toContain('ResponseAborted');

    // Status is 'aborted' — ChatExchange's `responseTerminated` then derives
    // `true` and forwards `terminated={true}` to QuestionBody, which renders
    // every option as a disabled button.
    expect(exchangeStatus(questionEx, '', false, false, false)).toBe('aborted');

    // The Layer-2 questionOvertaken flag also fires — defense in depth so
    // the buttons disable even if some other code path bypassed the status
    // check.
    expect(questionEx.questionOvertaken).toBe(true);
  });
});
