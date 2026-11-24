import { describe, it, expect } from 'vitest';
import { groupIntoExchanges, agentWorkedSince, exchangeResponseEvents, exchangeSteps, exchangeStatus, type StoredEvent, type ThreadEvent } from '../thread-events';

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
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'Do I need light and dark ones, and for what?', _eventId: 'msg-1', created: '2026-06-16T12:00:01Z' } as StoredEvent],
      [2, { type: 'ThoughtStreamed', text: 'Context: 1 tokens, 1 messages', request_event_id: 'msg-1', created: '2026-06-16T12:00:02Z' } as StoredEvent],
      [3, { type: 'TextStreamed', text: ANSWER_FIRST, request_event_id: 'msg-1', created: '2026-06-16T12:00:03Z' } as StoredEvent],
      [4, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-06-16T12:00:04Z' } as StoredEvent],
      [5, { type: 'UserQuestionAsked', tool_use_id: 'tu-outer-0', cc_session_id: '', question: 'So — which assets should I generate?', options: [{ id: 'a', label: 'Both' }], created: '2026-06-16T12:00:05Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['MessageReceived', 'UserQuestionAsked']);

    const mrEx = exchanges[0];
    expect(mrEx.steps.map(s => s.event.type)).toContain('TextStreamed');
    const mrResponse = exchangeResponseEvents(mrEx);
    expect(mrResponse.find(e => e.type === 'text' && e.md === ANSWER_FIRST)).toBeDefined();

    const divider = exchanges[1];
    expect(divider.steps.map(s => s.event.type)).not.toContain('TextStreamed');
    expect(divider.questionOvertaken).toBe(false);
  });

  it("a card's message, emitted after the ToolCalled, renders above a live card", () => {
    const MESSAGE = '1. Open https://example.com/settings\n2. Press **Save**.';
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'just tell me what to do with links', _eventId: 'msg-1', created: '2026-06-16T12:00:01Z' } as StoredEvent],
      [2, { type: 'ToolCalled', name: 'ask_user_question', args: { message: MESSAGE }, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-06-16T12:00:02Z' } as StoredEvent],
      [3, { type: 'TextStreamed', text: MESSAGE, request_event_id: 'msg-1', created: '2026-06-16T12:00:03Z' } as StoredEvent],
      [4, { type: 'UserQuestionAsked', tool_use_id: 'tu-outer#q0', cc_session_id: '', question: 'Tap when it is saved.', options: [{ id: 'a', label: 'Done, verify it' }, { id: 'b', label: 'Cannot find it' }], created: '2026-06-16T12:00:04Z' } as StoredEvent],
    ]);
    const [mrEx, divider] = groupIntoExchanges(events);
    expect(divider.userEvent.type).toBe('UserQuestionAsked');
    expect(exchangeResponseEvents(mrEx).find(e => e.type === 'text' && e.md === MESSAGE)).toBeDefined();
    expect(divider.steps.map(s => s.event.type)).not.toContain('TextStreamed');
    expect(divider.questionOvertaken).toBe(false);
  });

  it('chat agent post-answer text/response routes to the question exchange (rendered below the answer)', () => {
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
    expect(mrEx.steps.map(s => s.event.type)).toContain('ToolResult');
    const questionEx = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(questionEx.steps.map(s => s.event.type)).not.toContain('ToolResult');
    const steps = exchangeSteps(mrEx,  false,  false);
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

  it('the question\'s own CC ToolResult marks the divider as overtaken', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'do stuff' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu-q', cc_session_id: 's', question: 'X?', options: [{ id: 'a', label: 'X' }] }),
      ev(3, { type: 'CodingAgentToolResult', name: 'AskUserQuestion', result: 'rejected', tool_use_id: 'tu-q' }),
    );
    const divider = groupIntoExchanges(events).find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(divider.questionOvertaken).toBe(true);
  });

  it('any CC ToolResult after a permission card counts as the agent working past it', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'edit foo' }),
      ev(2, { type: 'CodingAgentPermissionRequest', request_id: 'r1', tool_use_id: 'tu', tool_name: 'Edit', input: { file_path: '/foo' }, summary: 'Edit /foo' }),
      ev(3, { type: 'CodingAgentToolResult', name: 'Bash', result: 'done', tool_use_id: 'tu-sibling' }),
    );
    const exchanges = groupIntoExchanges(events);
    const card = exchanges.findIndex(e => e.userEvent.type === 'CodingAgentPermissionRequest');
    expect(agentWorkedSince(exchanges, card)).toBe(true);
  });

  it('a late CC ToolResult of an earlier call leaves the divider live', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'do stuff' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu-q', cc_session_id: 's', question: 'X?', options: [{ id: 'a', label: 'X' }] }),
      ev(3, { type: 'CodingAgentToolResult', name: 'Bash', result: 'done', tool_use_id: 'tu-before-the-question' }),
    );
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(divider.questionOvertaken).toBe(false);
    expect(agentWorkedSince(exchanges, exchanges.indexOf(divider))).toBe(false);
  });

  it('chat TextStreamed after UserQuestionAsked marks divider as overtaken (chat symmetry)', () => {
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

    expect(divider.steps.map(s => s.event.type)).toContain('UserQuestionAnswered');
    expect(ctc.steps.map(s => s.event.type)).not.toContain('UserQuestionAnswered');

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
    expect(mr.steps.map(s => s.event.type)).toContain('ThoughtStreamed');
    const ctcTypes = ctc.steps.map(s => s.event.type);
    expect(ctcTypes).toContain('ToolCalled');
    expect(ctcTypes).toContain('TextStreamed');
    expect(ctcTypes).toContain('ResponseGenerated');
    expect(mr.steps.map(s => s.event.type)).not.toContain('ResponseGenerated');
    expect(mr.steps.map(s => s.event.type)).not.toContain('TextStreamed');
  });

  it('continuation moves below the card even when an injected user message redirected the turn first', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'make a gif', _eventId: 'msg-1', created: '2026-06-16T12:00:00Z' } as StoredEvent],
      [2, { type: 'ThoughtStreamed', text: 'thinking', request_event_id: 'msg-1', created: '2026-06-16T12:00:01Z' } as StoredEvent],
      [3, { type: 'MessageReceived', text: 'or any way', _eventId: 'msg-2', created: '2026-06-16T12:00:02Z' } as StoredEvent],
      [4, { type: 'PromptInjected', text: 'or any way', mode: 'human', injected_message_id: 'msg-2', request_event_id: 'msg-1', created: '2026-06-16T12:00:03Z' } as StoredEvent],
      [5, { type: 'ThoughtStreamed', text: 'thinking more', request_event_id: 'msg-1', created: '2026-06-16T12:00:04Z' } as StoredEvent],
      [6, { type: 'ChildThreadCompleted', child_thread_id: 'child-1', child_thread_title: 'icons', status: 'success', summary: 'icons landed', _eventId: 'ctc-1', created: '2026-06-16T12:00:05Z' } as StoredEvent],
      [7, { type: 'ToolCalled', name: 'run_python', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-06-16T12:00:06Z' } as StoredEvent],
      [8, { type: 'ResponseGenerated', text: 'rendered', request_event_id: 'msg-1', created: '2026-06-16T12:00:07Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['MessageReceived', 'MessageReceived', 'ChildThreadCompleted']);

    const injected = exchanges[1];
    const ctc = exchanges[2];
    expect(injected.steps.map(s => s.event.type)).not.toContain('ResponseGenerated');
    expect(injected.steps.map(s => s.event.type)).not.toContain('ToolCalled');
    expect(ctc.steps.map(s => s.event.type)).toContain('ToolCalled');
    expect(ctc.steps.map(s => s.event.type)).toContain('ResponseGenerated');
  });

  it('does NOT move the continuation when the turn is paused at a question divider (3e54cacb is preserved)', () => {
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
    expect(divider.steps.map(s => s.event.type)).toContain('ResponseGenerated');
    expect(ctc.steps.map(s => s.event.type)).not.toContain('ResponseGenerated');
    expect(exchangeStatus(divider, '', false, false, false)).toBe('done');
  });

  it('re-anchors the answered divider BELOW the intervening card so the live reply renders last', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'do the release', _eventId: 'msg-1', created: '2026-07-28T13:50:05Z' } as StoredEvent],
      [2, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T13:56:39Z' } as StoredEvent],
      [3, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'Re-fold onto current main?', options: [{ id: 'opt-0', label: 'Re-fold' }], channel: 'chat', created: '2026-07-28T13:56:40Z' } as StoredEvent],
      [4, { type: 'ChildThreadCompleted', child_thread_id: 'child-1', child_thread_title: 'Fixing Lucidos Installer Version Bug', status: 'success', summary: 'Hardening complete.', _eventId: 'ctc-1', created: '2026-07-28T14:06:50Z' } as StoredEvent],
      [5, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'opt-0' }, created: '2026-07-28T14:07:25Z' } as StoredEvent],
      [6, { type: 'ToolResult', name: 'ask_user_question', result: 'ok', tool_called_event_id: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T14:10:40Z' } as StoredEvent],
      [7, { type: 'TextStreamed', text: 'Re-fold approved. Clearing the old worktree…', request_event_id: 'msg-1', created: '2026-07-28T14:10:40Z' } as StoredEvent],
      [8, { type: 'ToolCalled', name: 'run_bash', args: {}, _eventId: 'tc-2', request_event_id: 'msg-1', created: '2026-07-28T14:11:09Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);

    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'ChildThreadCompleted',
      'UserQuestionAsked',
    ]);

    const ctc = exchanges[1];
    const divider = exchanges[2];

    const divTypes = divider.steps.map(s => s.event.type);
    expect(divTypes).toContain('UserQuestionAnswered');
    expect(divTypes).toContain('TextStreamed');
    expect(divTypes).toContain('ToolCalled');
    expect(ctc.steps.map(s => s.event.type)).not.toContain('TextStreamed');

    expect(exchangeStatus(ctc, '',  false, false, false,  false, false)).toBe('done');
    expect(exchangeStatus(divider, '',  true, false, false,  false, false)).toBe('streaming');
  });

  it('advances the redirect past an ALREADY-ANSWERED divider when a child completes mid-response', () => {
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

    expect(divider.steps.map(s => s.event.type)).toContain('UserQuestionAnswered');
    expect(divider.steps.filter(s => s.event.type === 'TextStreamed')).toHaveLength(1);
    const ctcTypes = ctc.steps.map(s => s.event.type);
    expect(ctcTypes).toContain('ToolCalled');
    expect(ctcTypes).toContain('TextStreamed');
    expect(exchangeStatus(ctc, '',  true, false, false,  false, false)).not.toBe('pending');
  });

  it('re-anchors a coding-agent permission divider to its grant, continuation included', () => {
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
      'ChildThreadCompleted',
      'CodingAgentPermissionRequest',
    ]);
    const divTypes = exchanges[2].steps.map(s => s.event.type);
    expect(divTypes).toContain('CodingAgentPermissionResolved');
    expect(divTypes).toContain('CodingAgentTextStreamed');
    expect(exchanges[1].steps.map(s => s.event.type)).not.toContain('CodingAgentTextStreamed');
  });

  it('leaves a stale coding-agent card in place when a cleanup sweep resolves it', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'edit foo', _eventId: 'msg-1', created: '2026-07-28T04:00:00Z' } as StoredEvent],
      [2, { type: 'CodingAgentPermissionRequest', request_id: 'r1', tool_use_id: 'tu', tool_name: 'Edit', input: {}, summary: 'Edit /foo', created: '2026-07-28T04:00:01Z' } as StoredEvent],
      [3, { type: 'ChildThreadCompleted', child_thread_id: 'c1', status: 'success', summary: 'x', _eventId: 'ctc-1', created: '2026-07-28T04:00:02Z' } as StoredEvent],
      [4, { type: 'CodingAgentTextStreamed', text: 'carrying on', created: '2026-07-28T04:00:03Z' } as StoredEvent],
      [5, { type: 'CodingAgentPermissionResolved', request_id: 'r1', allowed: false, reason: 'session ended', created: '2026-07-28T04:00:04Z' } as StoredEvent],
      [6, { type: 'CodingAgentIdled', created: '2026-07-28T04:00:05Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'CodingAgentPermissionRequest',
      'ChildThreadCompleted',
    ]);
    expect(exchanges[1].steps.map(s => s.event.type)).toContain('CodingAgentPermissionResolved');
    expect(exchanges[2].steps.map(s => s.event.type)).toContain('CodingAgentIdled');
  });

  it('re-anchors a coding-agent question divider to its answer, continuation included', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'ship it', _eventId: 'msg-1', created: '2026-07-28T04:00:00Z' } as StoredEvent],
      [2, { type: 'CodingAgentToolCalled', name: 'AskUserQuestion', tool_use_id: 'tu-q', input: {}, created: '2026-07-28T04:00:01Z' } as StoredEvent],
      [3, { type: 'UserQuestionAsked', tool_use_id: 'tu-q', cc_session_id: 's1', question: 'Which?', options: [{ id: 'a', label: 'A' }], created: '2026-07-28T04:00:02Z' } as StoredEvent],
      [4, { type: 'ChildThreadCompleted', child_thread_id: 'c1', status: 'success', summary: 'x', _eventId: 'ctc-1', created: '2026-07-28T04:00:03Z' } as StoredEvent],
      [5, { type: 'UserQuestionAnswered', tool_use_id: 'tu-q', answer: { kind: 'Selected', option_id: 'a' }, created: '2026-07-28T04:00:04Z' } as StoredEvent],
      [6, { type: 'CodingAgentTextStreamed', text: 'going with A', created: '2026-07-28T04:00:05Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual([
      'MessageReceived',
      'ChildThreadCompleted',
      'UserQuestionAsked',
    ]);
    const divTypes = exchanges[2].steps.map(s => s.event.type);
    expect(divTypes).toContain('UserQuestionAnswered');
    expect(divTypes).toContain('CodingAgentTextStreamed');
    expect(exchanges[1].steps.map(s => s.event.type)).not.toContain('CodingAgentTextStreamed');
  });
});

describe('continuationMoved — stale Thinking marker after a mid-flight handoff', () => {
  const HANDOFF = new Map<number, StoredEvent>([
    [1, { type: 'MessageReceived', text: 'do the release', _eventId: 'msg-1', created: '2026-07-28T20:14:00Z' } as StoredEvent],
    [2, { type: 'ThoughtStreamed', text: 'planning', request_event_id: 'msg-1', created: '2026-07-28T20:14:01Z' } as StoredEvent],
    [3, { type: 'ToolCalled', name: 'run_bash', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T20:14:02Z' } as StoredEvent],
    [4, { type: 'ToolResult', name: 'run_bash', result: 'ok', tool_called_event_id: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T20:14:03Z' } as StoredEvent],
    [5, { type: 'ThoughtStreamed', text: 'thinking again', request_event_id: 'msg-1', created: '2026-07-28T20:14:04Z' } as StoredEvent],
    [6, { type: 'ChildThreadCompleted', child_thread_id: 'child-1', child_thread_title: 'Fixing Duplicate Apple Notarization Submissions', status: 'canceled', summary: 'canceled', _eventId: 'ctc-1', created: '2026-07-28T20:15:03Z' } as StoredEvent],
    [7, { type: 'ToolCalled', name: 'run_bash', args: {}, _eventId: 'tc-2', request_event_id: 'msg-1', created: '2026-07-28T20:15:50Z' } as StoredEvent],
    [8, { type: 'ToolResult', name: 'run_bash', result: 'ok', tool_called_event_id: 'tc-2', request_event_id: 'msg-1', created: '2026-07-28T20:15:51Z' } as StoredEvent],
    [9, { type: 'ThoughtStreamed', text: 'still going', request_event_id: 'msg-1', created: '2026-07-28T20:15:52Z' } as StoredEvent],
  ]);

  it('drops the orphaned Thinking marker from the handed-off exchange while the turn runs on', () => {
    const exchanges = groupIntoExchanges(HANDOFF);
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['MessageReceived', 'ChildThreadCompleted']);
    const mr = exchanges[0];
    expect(mr.continuationMoved).toBe(true);

    const events = exchangeResponseEvents(mr,  false,  false);
    expect(events.filter(e => e.type === 'step' && e.outcome === 'pending')).toHaveLength(0);
    const last = events[events.length - 1];
    expect(last.type === 'step' && last.description === 'Thinking').toBe(false);
    expect(exchangeSteps(mr,  false,  false).filter(s => s.outcome === 'pending')).toHaveLength(0);
  });

  it('leaves the live Thinking marker alone in the exchange that took the continuation', () => {
    const exchanges = groupIntoExchanges(HANDOFF);
    const ctc = exchanges[1];
    expect(ctc.continuationMoved).toBeFalsy();
    const events = exchangeResponseEvents(ctc,  true,  false);
    expect(events.filter(e => e.type === 'step' && e.outcome === 'pending')).toHaveLength(1);
    expect(exchangeStatus(ctc, '',  true, false, false,  false, false)).toBe('streaming');
  });

  it('keeps a pending TOOL step spinning — its result can still re-route back by tool id', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'do it', _eventId: 'msg-1', created: '2026-07-28T09:00:00Z' } as StoredEvent],
      [2, { type: 'ToolCalled', name: 'ask_user_question', args: {}, description: 'Executing ask_user_question...', _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T09:00:01Z' } as StoredEvent],
      [3, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'q?', options: [{ id: 'a', label: 'A' }], created: '2026-07-28T09:00:02Z' } as StoredEvent],
    ]);
    const mr = groupIntoExchanges(events)[0];
    expect(mr.continuationMoved).toBe(true);
    const rendered = exchangeResponseEvents(mr,  false,  false);
    const pending = rendered.filter(e => e.type === 'step' && e.outcome === 'pending');
    expect(pending).toHaveLength(1);
    expect(pending[0].type === 'step' && pending[0].description).toBe('Executing ask_user_question...');
  });

  it('does NOT mark a turn whose follow-up is still queued — the loop is still working in it', () => {
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
    const rendered = exchangeResponseEvents(first,  false,  false);
    const pending = rendered.filter(e => e.type === 'step' && e.outcome === 'pending');
    expect(pending).toHaveLength(1);
    expect(pending[0].type === 'step' && pending[0].description).toBe('Thinking');
  });

  it('marks the exchange that actually owned the turn, not just the queued message in front of it', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'first', _eventId: 'msg-1', created: '2026-07-28T09:30:00Z' } as StoredEvent],
      [2, { type: 'ThoughtStreamed', text: 'planning', request_event_id: 'msg-1', created: '2026-07-28T09:30:01Z' } as StoredEvent],
      [3, { type: 'MessageReceived', text: 'second', _eventId: 'msg-2', created: '2026-07-28T09:30:02Z' } as StoredEvent],
      [4, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'q?', options: [{ id: 'a', label: 'A' }], created: '2026-07-28T09:30:03Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const first = exchanges.find(e => e.userEvent._eventId === 'msg-1')!;
    expect(first.continuationMoved).toBe(true);
    const rendered = exchangeResponseEvents(first,  false,  false);
    expect(rendered.filter(e => e.type === 'step' && e.outcome === 'pending')).toHaveLength(0);
  });

  it('clears the mark when the handed-off exchange takes the continuation back', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'first', _eventId: 'msg-1', created: '2026-07-28T09:20:00Z' } as StoredEvent],
      [2, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T09:20:01Z' } as StoredEvent],
      [3, { type: 'MessageReceived', text: 'second', _eventId: 'msg-2', created: '2026-07-28T09:20:02Z' } as StoredEvent],
      [4, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'q?', options: [{ id: 'a', label: 'A' }], created: '2026-07-28T09:20:03Z' } as StoredEvent],
      [5, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'a' }, created: '2026-07-28T09:20:04Z' } as StoredEvent],
      [6, { type: 'ToolResult', name: 'ask_user_question', result: 'a', tool_called_event_id: 'tc-1', request_event_id: 'msg-1', created: '2026-07-28T09:20:05Z' } as StoredEvent],
      [7, { type: 'PromptInjected', text: 'second', mode: 'human', injected_message_id: 'msg-2', request_event_id: 'msg-1', created: '2026-07-28T09:20:06Z' } as StoredEvent],
      [8, { type: 'ThoughtStreamed', text: 'on the follow-up now', request_event_id: 'msg-1', created: '2026-07-28T09:20:07Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const followup = exchanges.find(e => e.userEvent._eventId === 'msg-2')!;
    expect(followup.continuationMoved).toBeFalsy();
    const rendered = exchangeResponseEvents(followup,  true,  false);
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
  it('synthetic ToolResult lands in the same exchange as its ToolCalled', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'do it', _eventId: 'msg-1', created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [2, { type: 'TextStreamed', text: 'Want me to:', request_event_id: 'msg-1', created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [3, { type: 'ToolCalled', name: 'ask_user_question', args: {}, description: 'Executing ask_user_question...', _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [4, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'X?', options: [{ id: 'a', label: 'X' }], created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [5, { type: 'ResponseAborted', text: 'interrupted by engine restart', cause: 'engine_shutdown', request_event_id: 'msg-1', created: '2026-05-22T06:52:18Z' } as StoredEvent],
      [6, { type: 'ToolResult', name: 'ask_user_question', result: '[Tool execution interrupted by engine restart — original ToolCalled event_id: tc-1]', tool_called_event_id: 'tc-1', created: '2026-05-22T06:53:49Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const mrEx = exchanges.find(e => e.userEvent.type === 'MessageReceived')!;
    const mrSteps = mrEx.steps.map(s => s.event.type);
    expect(mrSteps).toContain('ToolCalled');
    expect(mrSteps).toContain('ToolResult');

    const questionEx = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(questionEx.steps.map(s => s.event.type)).not.toContain('ToolResult');

    const mrResponseEvents = exchangeResponseEvents(mrEx);
    const askStep = mrResponseEvents.find(e => e.type === 'step' && e.tool_name === 'ask_user_question');
    expect(askStep).toBeDefined();
    expect(askStep!.type === 'step' && askStep!.outcome).toBe('success');
  });

  it('synthetic ToolResult without tool_called_event_id falls back to current-exchange routing', () => {
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
  it('post-answer reply routes to the question divider, not the child card; divider is done, not aborted', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'toasts under keyboard', _eventId: 'msg-1', created: '2026-06-21T23:34:58Z' } as StoredEvent],
      [2, { type: 'TextStreamed', text: 'looking into it', request_event_id: 'msg-1', created: '2026-06-21T23:36:02Z' } as StoredEvent],
      [3, { type: 'ResponseGenerated', text: 'looking into it', request_event_id: 'msg-1', created: '2026-06-21T23:36:08Z' } as StoredEvent],
      [4, { type: 'ChildThreadCompleted', child_thread_id: 'child-1', child_thread_title: 'Fixing iOS Keyboard Toast Occlusion', status: 'canceled', summary: '', _eventId: 'ctc-1', created: '2026-06-21T23:36:18Z' } as StoredEvent],
      [5, { type: 'ThoughtStreamed', text: 'thinking', request_event_id: 'ctc-1', created: '2026-06-21T23:36:19Z' } as StoredEvent],
      [6, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'ctc-1', created: '2026-06-21T23:36:27Z' } as StoredEvent],
      [7, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'proceed?', options: [{ id: 'opt-0', label: 'Restart the agent' }], created: '2026-06-21T23:36:27Z' } as StoredEvent],
      [8, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'FreeText', text: 'Nei i handlelisye appen' }, created: '2026-06-21T23:36:32Z' } as StoredEvent],
      [9, { type: 'ToolResult', name: 'ask_user_question', result: 'ok', tool_called_event_id: 'tc-1', request_event_id: 'ctc-1', created: '2026-06-21T23:36:32Z' } as StoredEvent],
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

    const divTypes = divider.steps.map(s => s.event.type);
    expect(divTypes).toContain('UserQuestionAnswered');
    expect(divTypes).toContain('TextStreamed');
    expect(divTypes).toContain('ResponseGenerated');

    const ctcTypes = ctc.steps.map(s => s.event.type);
    expect(ctcTypes).not.toContain('TextStreamed');
    expect(ctcTypes).not.toContain('ResponseGenerated');

    expect(exchangeStatus(divider, '',  true, false, false,  true,  false)).toBe('done');
  });
});

describe('groupIntoExchanges — question asked while a queued follow-up message is pending', () => {
  it('post-answer reply routes to the divider, not the original message exchange; divider is done, not aborted', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'MessageReceived', text: 'Trenger vi håv?', _eventId: 'msg-1', created: '2026-06-22T14:50:00Z' } as StoredEvent],
      [2, { type: 'ThoughtStreamed', text: 'thinking', request_event_id: 'msg-1', created: '2026-06-22T14:50:01Z' } as StoredEvent],
      [3, { type: 'MessageReceived', text: 'Til hva?', _eventId: 'msg-2', created: '2026-06-22T14:50:05Z' } as StoredEvent],
      [4, { type: 'TextStreamed', text: 'Ja, en håv er verdt det.', request_event_id: 'msg-1', created: '2026-06-22T14:50:26Z' } as StoredEvent],
      [5, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'msg-1', created: '2026-06-22T14:50:26Z' } as StoredEvent],
      [6, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'Skal jeg legge til på handlelista?', options: [{ id: 'opt-0', label: 'Håv' }], multi_select: true, channel: 'chat', created: '2026-06-22T14:50:26Z' } as StoredEvent],
      [7, { type: 'QueuedMessageRemoved', removed_message_id: 'msg-2', created: '2026-06-22T15:07:44Z' } as StoredEvent],
      [8, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'MultiSelected', option_ids: [], text: 'Kan jeg bruke presenning som underlag?' }, created: '2026-06-22T15:08:15Z' } as StoredEvent],
      [9, { type: 'ToolResult', name: 'ask_user_question', result: 'ok', tool_called_event_id: 'tc-1', request_event_id: 'msg-1', created: '2026-06-22T15:08:15Z' } as StoredEvent],
      [10, { type: 'ThoughtStreamed', text: 'thinking', request_event_id: 'msg-1', created: '2026-06-22T15:08:33Z' } as StoredEvent],
      [11, { type: 'TextStreamed', text: 'Ja, en presenning fungerer fint.', request_event_id: 'msg-1', created: '2026-06-22T15:08:33Z' } as StoredEvent],
      [12, { type: 'ResponseGenerated', text: 'Ja, en presenning fungerer fint.', request_event_id: 'msg-1', created: '2026-06-22T15:08:33Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    const mr1 = exchanges.find(e => e.userEvent.type === 'MessageReceived' && e.userEvent._eventId === 'msg-1')!;

    const divTypes = divider.steps.map(s => s.event.type);
    expect(divTypes).toContain('UserQuestionAnswered');
    expect(divTypes).toContain('TextStreamed');
    expect(divTypes).toContain('ResponseGenerated');

    const mr1Types = mr1.steps.map(s => s.event.type);
    expect(mr1Types).toContain('ToolCalled');
    expect(mr1Types).toContain('ToolResult');
    expect(mr1Types).not.toContain('ResponseGenerated');

    expect(exchangeStatus(divider, '',  true, false, false,  true,  false)).toBe('done');
  });
});

describe('exchangeStatus + responseTerminated — UserQuestionAsked exchange aborted by engine restart', () => {
  it('ResponseAborted with the originating req_id terminates the UserQuestionAsked exchange', () => {
    const events = new Map<number, StoredEvent>([
      [1, { type: 'ChildThreadCompleted', child_thread_id: 'child-1', child_thread_title: 'child', status: 'success', summary: 'done', _eventId: 'ctc-1', created: '2026-05-22T06:51:27Z' } as StoredEvent],
      [2, { type: 'TextStreamed', text: 'Want me to:', request_event_id: 'ctc-1', created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [3, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'ctc-1', created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [4, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'Both pending Apply. Next?', options: [{ id: 'a', label: 'Apply both' }], created: '2026-05-22T06:51:40Z' } as StoredEvent],
      [5, { type: 'ResponseAborted', text: 'interrupted', cause: 'engine_shutdown', request_event_id: 'ctc-1', channel: 'chat', created: '2026-05-22T06:52:18Z' } as StoredEvent],
    ]);
    const exchanges = groupIntoExchanges(events);
    const questionEx = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;

    expect(questionEx.steps.map(s => s.event.type)).toContain('ResponseAborted');

    expect(exchangeStatus(questionEx, '', false, false, false)).toBe('aborted');

    expect(questionEx.questionOvertaken).toBe(true);
  });
});
