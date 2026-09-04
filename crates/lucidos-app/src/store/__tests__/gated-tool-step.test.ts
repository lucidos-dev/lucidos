// A tool call held by a permission card says so, and a refused one says that.
//
// Before `'blocked'` and `'denied'` existed, the row for a gated call lied in
// one of two directions, and which one you got was a race. It opened
// `'pending'`, so a command waiting on a human shimmered exactly like one that
// was running. A permission request also parks the thread at
// `waiting_for_user_answer`, which `isThreadQuiescent` counts as quiescent. The
// `threadIdle` sweep could therefore finalize the un-run row to a green
// `'success'`. A denial resolved to `'success'` too.
//
// Everything here drives the REAL fold, because the marks are written there:
// the request is an exchange starter, so the call it gates sits in the previous
// exchange and only `groupIntoExchanges` holds both ends. Both projections are
// asserted every time, since they are mirrors.
//
// See `docs/plans/2026-08-25-permission-blocked-step-state.md`.

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
import { rendersLiveStep } from '../event-rendering';
import { makeThreadState } from './thread-events-helpers';
import type { ResponseEvent, StepOutcome } from '../types';

function ev(seq: number, e: ThreadEvent): readonly [number, StoredEvent] {
  return [seq, { ...e, created: `2026-08-25T10:00:${String(seq).padStart(2, '0')}Z` } as StoredEvent];
}
const thread = (...entries: Array<readonly [number, StoredEvent]>) => new Map(entries);

/** Outcomes of both projections' step rows, keyed by the name each is known
 *  by, so a failure names which mirror drifted. */
function outcomes(exchange: Exchange, threadIdle = false): Record<string, StepOutcome[]> {
  return {
    exchangeSteps: exchangeSteps(exchange, false, threadIdle).map(s => s.outcome),
    exchangeResponseEvents: exchangeResponseEvents(exchange, false, threadIdle)
      .filter((e): e is Extract<ResponseEvent, { type: 'step' }> => e.type === 'step')
      .map(s => s.outcome),
  };
}

/** Assert both projections give the call rows exactly `expected`. */
function expectOutcomes(exchange: Exchange, expected: StepOutcome[], threadIdle = false): void {
  for (const [projection, actual] of Object.entries(outcomes(exchange, threadIdle))) {
    expect(actual, projection).toEqual(expected);
  }
}

/** The step rows of the inline projection, which is the one carrying results. */
function inlineRows(exchange: Exchange): Extract<ResponseEvent, { type: 'step' }>[] {
  return exchangeResponseEvents(exchange, false, false)
    .filter((e): e is Extract<ResponseEvent, { type: 'step' }> => e.type === 'step');
}

// The coding-agent lane, Claude Code and Codex alike. The request shares its
// `tool_use_id` with the call on both backends, so the fold pairs them through
// `toolCallOwners` across the divider between them.

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

/** The exchange holding the gated call: the one BEFORE the divider. */
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

  // The green-check half of the original bug, and the reason `'blocked'` had to
  // live outside `'pending'` rather than beside it. Every sweep keys on
  // `'pending'`, so a held row is exempt from them by construction.
  it('survives the quiescent sweep instead of being finalized to success', () => {
    expectOutcomes(callExchange(...upToRequest), ['blocked'], /* threadIdle */ true);
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

  // The refusal comes back to the agent as an ordinary tool result. Pairing by
  // id used to require the step still be `'pending'`. A denied row therefore
  // fell through to the walk, and a green check landed on it anyway.
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
  // The reported shape: one command raises a card while a sibling from the same
  // assistant message runs. Holding one must not mute the other.
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

// The chat lanes: the Lucidos Agent's command guard and its MCP gate. A chat
// `ToolCalled` carries no `tool_use_id`, so the fold takes the last call step
// of the exchange the request interrupted. That is exact, because the chat
// agentic loop runs one tool at a time.

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
        tool_called_event_id: 'tc-2',
        request_event_id: 'msg-1',
      } as ThreadEvent),
    );
    const rows = inlineRows(exchange);
    expect(rows.map(r => r.outcome)).toEqual(['denied']);
    expect(rows[0].result).toBe(DENIAL);
  });
});

describe('a queued follow-up between the call and the card', () => {
  // `MessageReceived` is an exchange starter, so a follow-up typed while the
  // agent works makes `current` that uningested MR. It holds no tool call, so
  // reading the interrupted turn off `current` finds nothing and the held row
  // keeps shimmering. `chatTurnOwner` reads `lastChatTurnReqId` instead, which
  // the fold already tracks for the divider redirect's version of this bug.
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
  // The whole point, checked across BOTH exchanges the card splits the turn
  // into. `shimmer-invariant.test.ts` asks the complementary question, that a
  // turn which IS working never loses its only shimmer.
  const events = thread(
    ev(1, { type: 'MessageReceived', text: PROMPT, channel: 'claude_code' } as ThreadEvent),
    ev(2, { type: 'SessionStarted', session_id: 's1', channel: 'claude_code' } as ThreadEvent),
    ccBash(3, 'tu-a', CMD),
    ccRequest(4, 'r1', 'tu-a', CMD),
  );

  it('neither the held call nor the card shimmers', () => {
    const exchanges = groupIntoExchanges(events);
    // The thread is parked, which is what makes `threadIdle` true here.
    const threadIdle = true;
    exchanges.forEach((exchange, i) => {
      const isLast = i === exchanges.length - 1;
      const rendered = exchangeResponseEvents(exchange, isLast, threadIdle);
      const status = exchangeStatus(exchange, '', isLast, false, true, threadIdle, true);
      const { className } = statusLabel(status, rendered.some(e => e.type === 'step'));
      const label = exchange.userEvent.type;
      expect(className, `${label} header`).not.toBe('working');
      expect(
        rendersLiveStep(true, false, rendered),
        `${label} steps`,
      ).toBe(false);
    });
  });

  it('and the card itself still reads as needing an answer', () => {
    const divider = groupIntoExchanges(events)[1];
    expect(exchangeStatus(divider, '', true, false, true, true, true)).toBe('awaiting-answer');
  });
});

describe('the incremental fold wakes the exchange it reached back into', () => {
  // The call's row lives in an exchange the request does not belong to, and
  // `ChatExchange` is memoized on a captured `revision`. Marking it without the
  // bump would leave the held row shimmering until something else re-rendered.
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
