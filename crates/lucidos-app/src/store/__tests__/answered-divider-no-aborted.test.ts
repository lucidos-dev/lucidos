import { describe, it, expect } from 'vitest';
import {
  exchangeStatus,
  groupIntoExchanges,
  handleEvent,
  makeOptimisticThreadState,
  type Exchange,
  type StoredEvent,
  type ThreadState,
} from '../thread-events';

const TS = '2026-06-16T17:36:27Z';

function answeredQuestionDivider(): Exchange {
  return {
    userEvent: {
      type: 'UserQuestionAsked',
      tool_use_id: 'tu-1',
      cc_session_id: '',
      question: 'Proceed?',
      options: [{ id: 'opt-2', label: 'Hold off' }],
    } as StoredEvent,
    userSeq: 3,
    steps: [
      { seq: 4, event: { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'opt-2' } } as StoredEvent },
    ],
  };
}

describe('answered divider never flashes Aborted during resume', () => {
  it('chat question divider with stale waiting status reads as resuming, not aborted', () => {
    const divider = answeredQuestionDivider();
    const status = exchangeStatus(divider, '',  true,  false,  false,  true,  true);
    expect(status).not.toBe('aborted');
  });

  it('CC question divider with stale waiting status reads coding-agent-working, not aborted', () => {
    const divider = answeredQuestionDivider();
    const status = exchangeStatus(divider, '', true, false,  true,  true,  true);
    expect(status).not.toBe('aborted');
    expect(status).toBe('coding-agent-working');
  });

  it('resolved permission divider with stale waiting status is not aborted', () => {
    const divider: Exchange = {
      userEvent: { type: 'CommandPermissionRequested', request_id: 'creq-1', tool_use_id: 'tu-1', tool_name: 'run_python', command: 'git pull', summary: 'pull' } as StoredEvent,
      userSeq: 2,
      steps: [
        { seq: 3, event: { type: 'CommandPermissionResolved', request_id: 'creq-1', allowed: true } as StoredEvent },
      ],
    };
    expect(exchangeStatus(divider, '', true, false, false,  true,  true)).not.toBe('aborted');
  });

  it('idle answered divider (status NOT waiting) settles done, not aborted', () => {
    const divider = answeredQuestionDivider();
    const status = exchangeStatus(divider, '', true, false, false,  true,  false);
    expect(status).toBe('done');
  });

  it('full chat ask_user_question answer→resume sequence ends done, never aborted', () => {
    const map = new Map<string, ThreadState>();
    const id = 'thread-1';
    map.set(id, makeOptimisticThreadState({ id, title: 'x', channel: 'chat', initiator: 'user', eventsLoaded: true, timestamp: TS, status: 'running' }));

    const seqEvents: Array<[number, StoredEvent, string]> = [
      [1, { type: 'MessageReceived', text: 'investigate', _eventId: 'mr-1' } as StoredEvent, TS],
      [2, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'mr-1' } as StoredEvent, TS],
      [3, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'Proceed?', options: [{ id: 'opt-2', label: 'Hold off' }], channel: 'chat' } as StoredEvent, TS],
      [4, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'opt-2' } } as StoredEvent, TS],
      [5, { type: 'ToolResult', name: 'ask_user_question', result: '{"Proceed?":"Hold off"}', success: true, tool_called_event_id: 'tc-1', request_event_id: 'mr-1' } as StoredEvent, TS],
      [6, { type: 'ThoughtStreamed', text: 'Context: 33931 tokens', request_event_id: 'mr-1' } as StoredEvent, TS],
      [7, { type: 'TextStreamed', text: 'OK, holding off.', request_event_id: 'mr-1' } as StoredEvent, '2026-06-16T17:36:32Z'],
      [8, { type: 'ResponseGenerated', text: 'OK, holding off.', request_event_id: 'mr-1' } as StoredEvent, '2026-06-16T17:36:32Z'],
    ];

    for (const [seq, event, created] of seqEvents) {
      handleEvent(map, id, seq, event, created);
      const thread = map.get(id)!;
      const exchanges = groupIntoExchanges(thread.events);
      const last = exchanges[exchanges.length - 1];
      const status = exchangeStatus(last, '',  true, false, false,  true,  true);
      expect(status, `prefix through seq ${seq}`).not.toBe('aborted');
    }

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    const last = exchanges[exchanges.length - 1];
    expect(last.userEvent.type).toBe('UserQuestionAsked');
    expect(exchangeStatus(last, '', true, false, false, true, false)).toBe('done');
  });
});
