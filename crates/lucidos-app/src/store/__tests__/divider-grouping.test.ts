import { describe, it, expect } from 'vitest';
import { groupIntoExchanges, exchangeResponseEvents, exchangeStatus, type StoredEvent, type ThreadEvent } from '../thread-events';

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

  it('answered question divider with CC resume is cc-working', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'hi' }),
      ev(2, { type: 'UserQuestionAsked', tool_use_id: 'tu1', cc_session_id: 's', question: 'q', options: [] }),
      ev(3, { type: 'UserQuestionAnswered', tool_use_id: 'tu1', answer: { kind: 'Selected', option_id: 'a' } }),
      ev(4, { type: 'CodingAgentTextStreamed', text: 'continuing' }),
    );
    const exchanges = groupIntoExchanges(events);
    const divider = exchanges[1];
    expect(exchangeStatus(divider, '', true, false, true)).toBe('cc-working');
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
    const aSteps = exchangeResponseEvents(exA, 0, false, false).filter(e => e.type === 'step');
    const ghStep = aSteps.find(s => s.tool_name === 'Bash');
    expect(ghStep?.success).toBe(true);

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
    expect(step?.success).toBe(true);
  });
});
