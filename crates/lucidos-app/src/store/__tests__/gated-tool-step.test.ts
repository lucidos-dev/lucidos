
import { describe, it, expect } from 'vitest';
import {
  computeExchanges,
  groupIntoExchanges,
  exchangeResponseEvents,
  exchangeStatus,
  exchangeSteps,
  type Exchange,
  type StoredEvent,
  type ThreadEvent,
} from '../thread-events';
import { statusLabel } from '../exchange-status';
import { liveStepInBody, responseBody } from '../event-rendering';
import { makeThreadState } from './thread-events-helpers';
import type { ResponseEvent, StepOutcome } from '../types';

function ev(seq: number, e: ThreadEvent): readonly [number, StoredEvent] {
  return [seq, { ...e, created: `2026-08-25T10:00:${String(seq).padStart(2, '0')}Z` } as StoredEvent];
}
const thread = (...entries: Array<readonly [number, StoredEvent]>) => new Map(entries);

function outcomes(exchange: Exchange, threadIdle = false): Record<string, StepOutcome[]> {
  return {
    exchangeSteps: exchangeSteps(exchange, false, threadIdle).map(s => s.outcome),
    exchangeResponseEvents: exchangeResponseEvents(exchange, false, threadIdle)
      .filter((e): e is Extract<ResponseEvent, { type: 'step' }> => e.type === 'step')
      .map(s => s.outcome),
  };
}

function expectOutcomes(exchange: Exchange, expected: StepOutcome[], threadIdle = false): void {
  for (const [projection, actual] of Object.entries(outcomes(exchange, threadIdle))) {
    expect(actual, projection).toEqual(expected);
  }
}

function inlineRows(exchange: Exchange): Extract<ResponseEvent, { type: 'step' }>[] {
  return exchangeResponseEvents(exchange, false, false)
    .filter((e): e is Extract<ResponseEvent, { type: 'step' }> => e.type === 'step');
}


const ccBash = (seq: number, id: string, command: string) =>
  ev(seq, { type: 'CodingAgentToolCalled', name: 'Bash', args: { command }, tool_use_id: id });
const ccRequest = (seq: number, requestId: string, id: string, command: string) =>
  ev(seq, {
    type: 'CodingAgentPermissionRequest',
    request_id: requestId,
    tool_use_id: id,
    tool_name: 'Bash',
    input: { command },
    summary: `Bash ${command}`,
  });
const ccResolved = (seq: number, requestId: string, allowed: boolean) =>
  ev(seq, { type: 'CodingAgentPermissionResolved', request_id: requestId, allowed });
const ccResult = (seq: number, id: string, result: string) =>
  ev(seq, { type: 'CodingAgentToolResult', name: 'Bash', result, tool_use_id: id });

const PROMPT = 'read the PR report';
const CMD = 'cd /tmp/report && python3 -c "import json"';
const DENIAL = "The user doesn't want to proceed with this tool use.";

function callExchange(...entries: Array<readonly [number, StoredEvent]>): Exchange {
  return groupIntoExchanges(thread(
    ev(1, { type: 'MessageReceived', text: PROMPT }),
    ...entries,
  ))[0];
}

describe('a coding-agent call held by a permission card', () => {
  const upToRequest = [ccBash(2, 'tu-a', CMD), ccRequest(3, 'r1', 'tu-a', CMD)] as const;

  it('reads blocked, not in-progress', () => {
    expectOutcomes(callExchange(...upToRequest), ['blocked']);
  });

  it('survives the quiescent sweep instead of being finalized to success', () => {
    expectOutcomes(callExchange(...upToRequest), ['blocked'],  true);
  });

  it('the card is still its own divider exchange', () => {
    const exchanges = groupIntoExchanges(thread(
      ev(1, { type: 'MessageReceived', text: PROMPT }),
      ...upToRequest,
    ));
    expect(exchanges.map(e => e.userEvent.type))
      .toEqual(['MessageReceived', 'CodingAgentPermissionRequest']);
  });

  it('allowing it hands the row back to the ordinary shimmer', () => {
    expectOutcomes(callExchange(...upToRequest, ccResolved(4, 'r1', true)), ['pending']);
  });

  it('and the result then ticks it off as usual', () => {
    const exchange = callExchange(
      ...upToRequest,
      ccResolved(4, 'r1', true),
      ccResult(5, 'tu-a', 'KEYS: [...]'),
    );
    expectOutcomes(exchange, ['success']);
  });

  it('denying it ends the row at denied', () => {
    expectOutcomes(callExchange(...upToRequest, ccResolved(4, 'r1', false)), ['denied']);
  });

  it('and the refusal that follows does not turn it back into a success', () => {
    const exchange = callExchange(
      ...upToRequest,
      ccResolved(4, 'r1', false),
      ccResult(5, 'tu-a', DENIAL),
    );
    expectOutcomes(exchange, ['denied']);
  });

  it('the refusal text lands on the refused row, so the detail explains it', () => {
    const exchange = callExchange(
      ...upToRequest,
      ccResolved(4, 'r1', false),
      ccResult(5, 'tu-a', DENIAL),
    );
    expect(inlineRows(exchange)[0].result).toBe(DENIAL);
  });
});

describe('a gated call beside a running one', () => {
  const parallel = [
    ccBash(2, 'tu-a', CMD),
    ccBash(3, 'tu-b', 'git show HEAD^{commit}'),
    ccRequest(4, 'r1', 'tu-a', CMD),
  ] as const;

  it('holds only the call the card is about', () => {
    expectOutcomes(callExchange(...parallel), ['blocked', 'pending']);
  });

  it('the sibling finishing does not disturb the held row', () => {
    expectOutcomes(callExchange(...parallel, ccResult(5, 'tu-b', 'ok')), ['blocked', 'success']);
  });

  it('and denying the held one leaves the sibling alone', () => {
    const exchange = callExchange(
      ...parallel,
      ccResult(5, 'tu-b', 'ok'),
      ccResolved(6, 'r1', false),
      ccResult(7, 'tu-a', DENIAL),
    );
    expectOutcomes(exchange, ['denied', 'success']);
  });
});


const chatCall = (seq: number, name: string) =>
  ev(seq, {
    type: 'ToolCalled',
    name,
    args: {},
    _eventId: `tc-${seq}`,
    request_event_id: 'msg-1',
  } as ThreadEvent);

function chatCallExchange(...entries: Array<readonly [number, StoredEvent]>): Exchange {
  return groupIntoExchanges(thread(
    ev(1, { type: 'MessageReceived', text: 'search slack', _eventId: 'msg-1' } as ThreadEvent),
    ...entries,
  ))[0];
}

describe('the chat MCP gate holds its call the same way', () => {
  const mcpRequest = (seq: number) =>
    ev(seq, {
      type: 'McpPermissionRequested',
      request_id: 'm1',
      tool_use_id: 'tu-m',
      server_id: 'slack',
      server_name: 'Slack',
      tool_name: 'conversations_search_messages',
      arguments_summary: '{}',
    });
  const upToRequest = [chatCall(2, 'mcp__slack__search'), mcpRequest(3)] as const;

  it('reads blocked while the card is open', () => {
    expectOutcomes(chatCallExchange(...upToRequest), ['blocked']);
  });

  it('allowing it returns the row to pending', () => {
    const exchange = chatCallExchange(
      ...upToRequest,
      ev(4, { type: 'McpPermissionResolved', request_id: 'm1', allowed: true }),
    );
    expectOutcomes(exchange, ['pending']);
  });

  it('denying it ends the row at denied', () => {
    const exchange = chatCallExchange(
      ...upToRequest,
      ev(4, { type: 'McpPermissionResolved', request_id: 'm1', allowed: false }),
    );
    expectOutcomes(exchange, ['denied']);
  });

  it('the refusal lands on the refused row rather than an older one', () => {
    const exchange = chatCallExchange(
      ...upToRequest,
      ev(4, { type: 'McpPermissionResolved', request_id: 'm1', allowed: false }),
      ev(5, {
        type: 'ToolResult',
        name: 'mcp__slack__search',
        result: DENIAL,
        success: false,
        tool_called_event_id: 'tc-2',
        request_event_id: 'msg-1',
      } as ThreadEvent),
    );
    expectOutcomes(exchange, ['denied']);
    expect(inlineRows(exchange)[0].result).toBe(DENIAL);
  });
});

describe('a queued follow-up between the call and the card', () => {
  const events = thread(
    ev(1, { type: 'MessageReceived', text: 'search slack', _eventId: 'msg-1' } as ThreadEvent),
    chatCall(2, 'mcp__slack__search'),
    ev(3, { type: 'MessageReceived', text: 'actually never mind', _eventId: 'msg-2' } as ThreadEvent),
    ev(4, {
      type: 'McpPermissionRequested',
      request_id: 'm1',
      tool_use_id: 'tu-m',
      server_id: 'slack',
      server_name: 'Slack',
      tool_name: 'conversations_search_messages',
      arguments_summary: '{}',
    }),
  );

  it('still marks the call, in the turn that made it', () => {
    const exchanges = groupIntoExchanges(events);
    const turn = exchanges.find(e => e.userEvent._eventId === 'msg-1');
    expect(turn, 'the originating turn').toBeDefined();
    expectOutcomes(turn!, ['blocked']);
  });

  it('and marks nothing on the queued follow-up', () => {
    const queued = groupIntoExchanges(events).find(e => e.userEvent._eventId === 'msg-2');
    expect(queued?.blockedStepSeqs).toBeUndefined();
  });
});

describe('the chat command guard holds its call the same way', () => {
  const commandRequest = (seq: number) =>
    ev(seq, {
      type: 'CommandPermissionRequested',
      request_id: 'c1',
      tool_use_id: 'tu-c',
      tool_name: 'run_bash',
      command: 'rm -rf /tmp/x',
      summary: 'May delete files.',
    });
  const held = [chatCall(2, 'run_bash'), commandRequest(3)] as const;

  it('reads blocked while the card is open', () => {
    expectOutcomes(chatCallExchange(...held), ['blocked']);
  });

  it('and takes the decision', () => {
    const denied = ev(4, { type: 'CommandPermissionResolved', request_id: 'c1', allowed: false });
    expectOutcomes(chatCallExchange(...held, denied), ['denied']);
  });
});

describe('a thread with no permission card', () => {
  it('gains no marks and renders exactly as before', () => {
    const exchange = callExchange(ccBash(2, 'tu-a', CMD), ccResult(3, 'tu-a', 'ok'));
    expect(exchange.blockedStepSeqs).toBeUndefined();
    expect(exchange.deniedStepSeqs).toBeUndefined();
    expectOutcomes(exchange, ['success']);
  });
});

describe('nothing on screen claims the machine is busy while a card is open', () => {
  const events = thread(
    ev(1, { type: 'MessageReceived', text: PROMPT, channel: 'claude_code' } as ThreadEvent),
    ev(2, { type: 'SessionStarted', session_id: 's1', channel: 'claude_code' } as ThreadEvent),
    ccBash(3, 'tu-a', CMD),
    ccRequest(4, 'r1', 'tu-a', CMD),
  );

  it('neither the held call nor the card shimmers', () => {
    const exchanges = groupIntoExchanges(events);
    const threadIdle = true;
    exchanges.forEach((exchange, i) => {
      const isLast = i === exchanges.length - 1;
      const rendered = exchangeResponseEvents(exchange, isLast, threadIdle);
      const status = exchangeStatus(exchange, '', isLast, false, true, threadIdle, true);
      const { className } = statusLabel(status, rendered.some(e => e.type === 'step'));
      const label = exchange.userEvent.type;
      expect(className, `${label} header`).not.toBe('working');
      expect(
        liveStepInBody(responseBody(rendered, { showSteps: true, showDetails: true, rowsHidden: 0 })),
        `${label} steps`,
      ).toBe(-1);
    });
  });

  it('and the card itself still reads as needing an answer', () => {
    const divider = groupIntoExchanges(events)[1];
    expect(exchangeStatus(divider, '', true, false, true, true, true)).toBe('awaiting-answer');
  });
});

describe('the incremental fold wakes the exchange it reached back into', () => {
  it('bumps the prior exchange revision when the card arrives', () => {
    const state = makeThreadState();
    for (const [seq, event] of [
      ev(1, { type: 'MessageReceived', text: PROMPT }),
      ev(2, { type: 'SessionStarted', session_id: 's1', channel: 'claude_code' } as ThreadEvent),
      ccBash(3, 'tu-a', CMD),
    ]) {
      state.events.set(seq, event);
      computeExchanges(state);
    }
    const before = computeExchanges(state)[0].revision ?? 0;

    const [seq, event] = ccRequest(4, 'r1', 'tu-a', CMD);
    state.events.set(seq, event);
    const after = computeExchanges(state)[0];

    expect(after.revision ?? 0).toBeGreaterThan(before);
    expectOutcomes(after, ['blocked']);
  });
});
