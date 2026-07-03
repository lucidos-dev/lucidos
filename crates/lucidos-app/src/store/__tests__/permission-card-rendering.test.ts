import { describe, it, expect } from 'vitest';
import { exchangeStatus, groupIntoExchanges, type Exchange } from '../thread-events';
import type { StoredEvent } from '../thread-events';

function step(seq: number, event: Partial<StoredEvent> & { type: string }): { seq: number; event: StoredEvent } {
  return { seq, event: event as StoredEvent };
}

function exchange(steps: Array<{ seq: number; event: StoredEvent }>): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'edit my skill file' } as StoredEvent,
    userSeq: 0,
    steps,
  };
}

function permissionRequestStep(seq: number, overrides: Partial<{
  request_id: string;
  tool_use_id: string;
  tool_name: string;
  input: Record<string, unknown>;
  summary: string;
}> = {}) {
  return step(seq, {
    type: 'CodingAgentPermissionRequest',
    request_id: 'req-1',
    tool_use_id: 'tu_1',
    tool_name: 'Edit',
    input: {},
    summary: 'Edit /tmp/x',
    ...overrides,
  });
}

describe('exchangeStatus around CodingAgentPermissionRequest', () => {
  it('exchangeStatus reads as awaiting-answer while waiting for permission (no spinner, no Done label)', () => {
    const ex = exchange([
      step(1, { type: 'SessionStarted', session_id: 'sess', branch: '' }),
      step(2, { type: 'CodingAgentTextStreamed', text: 'editing…' }),
      permissionRequestStep(3, { request_id: 'req-3' }),
    ]);
    expect(exchangeStatus(ex, '', true, false, true)).toBe('awaiting-answer');
  });

  it('exchangeStatus returns to coding-agent-working once CC resumes after answer', () => {
    const ex = exchange([
      step(1, { type: 'SessionStarted', session_id: 'sess', branch: '' }),
      permissionRequestStep(2, { request_id: 'req-4' }),
      step(3, { type: 'CodingAgentPermissionResolved', request_id: 'req-4', allowed: true }),
      step(4, { type: 'CodingAgentTextStreamed', text: 'continuing…' }),
    ]);
    expect(exchangeStatus(ex, '', true, false, true)).toBe('coding-agent-working');
  });
});

function commandRequestStep(seq: number, overrides: Partial<{
  request_id: string;
  tool_use_id: string;
  tool_name: string;
  command: string;
  summary: string;
}> = {}) {
  return step(seq, {
    type: 'CommandPermissionRequested',
    request_id: 'creq-1',
    tool_use_id: 'tu_1',
    tool_name: 'run_bash',
    command: 'curl -X POST https://api.example.com/charge',
    summary: 'May perform a mutating HTTP request.',
    ...overrides,
  });
}

describe('exchangeStatus around CommandPermissionRequested (chat command guard)', () => {
  it('reads as awaiting-answer while waiting for the command permission card', () => {
    const ex = exchange([
      step(1, { type: 'ToolCalled', name: 'run_bash', description: 'curl …' }),
      commandRequestStep(2, { request_id: 'creq-3' }),
    ]);
    // threadIsCC = false — the command guard fires on chat threads.
    expect(exchangeStatus(ex, '', true, false, false)).toBe('awaiting-answer');
  });

  it('leaves awaiting-answer once the command permission is resolved', () => {
    const ex = exchange([
      commandRequestStep(1, { request_id: 'creq-4' }),
      step(2, { type: 'CommandPermissionResolved', request_id: 'creq-4', allowed: true }),
      step(3, { type: 'ToolResult', name: 'run_bash', result: 'ok' }),
      step(4, { type: 'ResponseGenerated', text: 'done' }),
    ]);
    expect(exchangeStatus(ex, '', true, false, false)).not.toBe('awaiting-answer');
  });

  // The grant resumes the turn: the agent's post-grant reply routes into the
  // divider (below the card) via reqIdRedirect, so a completed divider owns the
  // turn's ResponseGenerated and reads 'done' — even at idle.
  it('resolved command-permission divider with its routed reply reads as done', () => {
    const divider: Exchange = {
      userEvent: commandRequestStep(1, { request_id: 'creq-5' }).event,
      userSeq: 1,
      steps: [
        step(2, { type: 'CommandPermissionResolved', request_id: 'creq-5', allowed: true, persist_scope: 'broad' }),
        step(3, { type: 'TextStreamed', text: 'done' }),
        step(4, { type: 'ResponseGenerated', text: 'done' }),
      ],
    };
    // isLast=true, threadIdle=true.
    expect(exchangeStatus(divider, '', true, false, false, true)).toBe('done');
  });

  it('routes post-grant work into the divider so the reply renders below the card', () => {
    const ev = (type: string, fields: Record<string, unknown> = {}): StoredEvent =>
      ({ type, ...fields }) as StoredEvent;
    const events = new Map<number, StoredEvent>([
      [1, ev('MessageReceived', { text: 'Run this bash command for me: curl …', _eventId: 'req-1' })],
      [2, ev('ToolCalled', { name: 'run_bash', args: { command: 'curl …' }, request_event_id: 'req-1', _eventId: 'tc-1' })],
      [3, ev('CommandPermissionRequested', { request_id: 'creq-1', tool_use_id: 'tu-1', tool_name: 'run_bash', command: 'curl …', summary: 'Sends a POST request.', _eventId: 'cpr-1' })],
      [4, ev('CommandPermissionResolved', { request_id: 'creq-1', allowed: true, persist_scope: 'broad' })],
      [5, ev('ToolResult', { name: 'run_bash', result: 'ok', success: true, tool_called_event_id: 'tc-1', request_event_id: 'req-1' })],
      [6, ev('TextStreamed', { text: 'The request succeeded.', request_event_id: 'req-1' })],
      [7, ev('ResponseGenerated', { text: 'The request succeeded.', request_event_id: 'req-1' })],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);

    const [msg, divider] = exchanges;
    expect(msg.userEvent.type).toBe('MessageReceived');
    expect(divider.userEvent.type).toBe('CommandPermissionRequested');
    // The gated tool call + its result stay in the message exchange (the call
    // was initiated before the card; its result re-routes there by
    // tool_called_event_id, so the "Running …" step renders merged above the
    // card). Everything emitted AFTER the grant routes into the divider, so the
    // reply renders BELOW the permission card — not above it.
    expect(msg.steps.map(s => s.event.type)).toEqual(['ToolCalled', 'ToolResult']);
    expect(divider.steps.map(s => s.event.type)).toEqual([
      'CommandPermissionResolved', 'TextStreamed', 'ResponseGenerated',
    ]);

    // The divider now owns the turn's terminal → reads 'done'; the message
    // exchange has no terminal of its own → 'interrupted' ("Done ↳").
    expect(exchangeStatus(divider, '', true, false, false, true)).toBe('done');
    expect(exchangeStatus(msg, '', false, false, false, true)).toBe('interrupted');
  });

  // Mirrors the reported screenshot: the agent thinks, runs a gated command,
  // the card resolves, then it keeps thinking and replies. All post-grant
  // progress must land in the divider (below the card), never back in the
  // message exchange (above the card).
  it('keeps post-grant thinking + reply below the card across a python git pull', () => {
    const ev = (type: string, fields: Record<string, unknown> = {}): StoredEvent =>
      ({ type, ...fields }) as StoredEvent;
    const events = new Map<number, StoredEvent>([
      [1, ev('MessageReceived', { text: 'map the tree format', _eventId: 'req-1' })],
      [2, ev('ThoughtStreamed', { text: 'Let me pull the latest from git first.', request_event_id: 'req-1' })],
      [3, ev('ToolCalled', { name: 'run_python', args: {}, request_event_id: 'req-1', _eventId: 'tc-1' })],
      [4, ev('CommandPermissionRequested', { request_id: 'creq-1', tool_use_id: 'tu-1', tool_name: 'run_python', command: 'git pull', summary: 'Pulls from a git repo.', _eventId: 'cpr-1' })],
      [5, ev('CommandPermissionResolved', { request_id: 'creq-1', allowed: true, persist_scope: 'session' })],
      [6, ev('ToolResult', { name: 'run_python', result: 'Already up to date', success: true, tool_called_event_id: 'tc-1', request_event_id: 'req-1' })],
      [7, ev('ThoughtStreamed', { text: 'Now map the fields.', request_event_id: 'req-1' })],
      [8, ev('TextStreamed', { text: 'Here are the fields …', request_event_id: 'req-1' })],
      [9, ev('ResponseGenerated', { text: 'Here are the fields …', request_event_id: 'req-1' })],
    ]);
    const [msg, divider] = groupIntoExchanges(events);
    // Pre-grant thinking + the gated call/result stay above the card.
    expect(msg.steps.map(s => s.event.type)).toEqual([
      'ThoughtStreamed', 'ToolCalled', 'ToolResult',
    ]);
    // Post-grant thinking + the reply render below the card.
    expect(divider.steps.map(s => s.event.type)).toEqual([
      'CommandPermissionResolved', 'ThoughtStreamed', 'TextStreamed', 'ResponseGenerated',
    ]);
  });

  // A genuine crash after the grant (engine died before the agent resumed)
  // leaves the divider holding only the resolution. At idle that must read
  // 'aborted', not a misleading 'done' — the work never completed.
  it('command-permission divider with only the resolution reads aborted at idle', () => {
    const divider: Exchange = {
      userEvent: commandRequestStep(1, { request_id: 'creq-6' }).event,
      userSeq: 1,
      steps: [
        step(2, { type: 'CommandPermissionResolved', request_id: 'creq-6', allowed: true }),
      ],
    };
    expect(exchangeStatus(divider, '', true, false, false, true)).toBe('aborted');
  });
});

function mcpRequestStep(seq: number, overrides: Partial<{
  request_id: string;
  tool_use_id: string;
  server_id: string;
  server_name: string;
  tool_name: string;
  arguments_summary: string;
}> = {}) {
  return step(seq, {
    type: 'McpPermissionRequested',
    request_id: 'mreq-1',
    tool_use_id: 'tu_1',
    server_id: 'slack',
    server_name: 'Slack (read-only)',
    tool_name: 'channels_list',
    arguments_summary: '{ "query": "ua-tech" }',
    ...overrides,
  });
}

describe('exchangeStatus around McpPermissionRequested (chat MCP gate)', () => {
  it('reads as awaiting-answer while waiting for the MCP permission card', () => {
    const ex = exchange([
      step(1, { type: 'ToolCalled', name: 'mcp__slack__channels_list', description: 'channels_list' }),
      mcpRequestStep(2, { request_id: 'mreq-3' }),
    ]);
    // threadIsCC = false — the MCP gate fires on chat threads.
    expect(exchangeStatus(ex, '', true, false, false)).toBe('awaiting-answer');
  });

  it('leaves awaiting-answer once the MCP permission is resolved', () => {
    const ex = exchange([
      mcpRequestStep(1, { request_id: 'mreq-4' }),
      step(2, { type: 'McpPermissionResolved', request_id: 'mreq-4', allowed: true }),
      step(3, { type: 'ToolResult', name: 'mcp__slack__channels_list', result: 'ok' }),
      step(4, { type: 'ResponseGenerated', text: 'done' }),
    ]);
    expect(exchangeStatus(ex, '', true, false, false)).not.toBe('awaiting-answer');
  });

  // The grant resumes the turn: the agent's post-grant reply routes into the
  // divider (below the card) via reqIdRedirect, so a completed divider owns the
  // turn's ResponseGenerated and reads 'done'.
  it('resolved MCP-permission divider with its routed reply reads as done', () => {
    const divider: Exchange = {
      userEvent: mcpRequestStep(1, { request_id: 'mreq-5' }).event,
      userSeq: 1,
      steps: [
        step(2, { type: 'McpPermissionResolved', request_id: 'mreq-5', allowed: true, persist_scope: 'broad' }),
        step(3, { type: 'TextStreamed', text: 'done' }),
        step(4, { type: 'ResponseGenerated', text: 'done' }),
      ],
    };
    expect(exchangeStatus(divider, '', true, false, false, true)).toBe('done');
  });

  it('routes post-grant work into the divider so the reply renders below the card', () => {
    const ev = (type: string, fields: Record<string, unknown> = {}): StoredEvent =>
      ({ type, ...fields }) as StoredEvent;
    const events = new Map<number, StoredEvent>([
      [1, ev('MessageReceived', { text: 'list the ua-tech channels', _eventId: 'req-1' })],
      [2, ev('ToolCalled', { name: 'mcp__slack__channels_list', args: { query: 'ua-tech' }, request_event_id: 'req-1', _eventId: 'tc-1' })],
      [3, ev('McpPermissionRequested', { request_id: 'mreq-1', tool_use_id: 'tu-1', server_id: 'slack', server_name: 'Slack (read-only)', tool_name: 'channels_list', arguments_summary: '{}', _eventId: 'mpr-1' })],
      [4, ev('McpPermissionResolved', { request_id: 'mreq-1', allowed: true, persist_scope: 'broad' })],
      [5, ev('ToolResult', { name: 'mcp__slack__channels_list', result: 'ok', success: true, tool_called_event_id: 'tc-1', request_event_id: 'req-1' })],
      [6, ev('TextStreamed', { text: 'Found 3 channels.', request_event_id: 'req-1' })],
      [7, ev('ResponseGenerated', { text: 'Found 3 channels.', request_event_id: 'req-1' })],
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);

    const [msg, divider] = exchanges;
    expect(msg.userEvent.type).toBe('MessageReceived');
    expect(divider.userEvent.type).toBe('McpPermissionRequested');
    // The gated tool call + its result stay in the message exchange; everything
    // emitted AFTER the grant routes into the divider (below the card).
    expect(msg.steps.map(s => s.event.type)).toEqual(['ToolCalled', 'ToolResult']);
    expect(divider.steps.map(s => s.event.type)).toEqual([
      'McpPermissionResolved', 'TextStreamed', 'ResponseGenerated',
    ]);

    expect(exchangeStatus(divider, '', true, false, false, true)).toBe('done');
    expect(exchangeStatus(msg, '', false, false, false, true)).toBe('interrupted');
  });
});
