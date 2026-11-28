
import { describe, it, expect } from 'vitest';
import type { Exchange } from '../thread-events';
import { queuedFollowupRun } from '../thread-events';
import type { SequencedEvent, StoredEvent } from '../thread-events/thread-event-types';

const ev = (seq: number, e: StoredEvent): SequencedEvent => ({ seq, event: e });

function userMsg(seq: number, text: string, persisted: boolean): Exchange {
  return {
    userEvent: persisted
      ? ({ type: 'MessageReceived', text, _eventId: `m${seq}`, created: `t${seq}` } as StoredEvent)
      : ({ type: 'MessageReceived', text, _eventId: `m${seq}`, _displayCreated: `t${seq}` } as StoredEvent),
    userSeq: seq,
    steps: [],
  };
}

function answeredQuestionNoTerminal(seq: number): Exchange {
  return {
    userEvent: { type: 'UserQuestionAsked', tool_use_id: `q${seq}`, _eventId: `q${seq}` } as StoredEvent,
    userSeq: seq,
    steps: [
      ev(seq + 1, { type: 'UserQuestionAnswered', tool_use_id: `q${seq}` } as StoredEvent),
      ev(seq + 2, { type: 'ToolCalled', name: 'x' } as StoredEvent),
      ev(seq + 3, { type: 'ToolResult', name: 'x' } as StoredEvent),
    ],
  };
}

function completedTurn(seq: number, text: string): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text, _eventId: `m${seq}`, created: `t${seq}` } as StoredEvent,
    userSeq: seq,
    steps: [ev(seq + 1, { type: 'ResponseGenerated', text: 'ok' } as StoredEvent)],
  };
}

describe('follow-up to an idle thread is the active turn, never queued in history', () => {
  it('does not queue the follow-up behind a stale answered-question divider', () => {
    const exchanges = [
      answeredQuestionNoTerminal(10),
      completedTurn(20, 'second'),
      userMsg(30, 'my follow-up', true),
    ];
    const run = queuedFollowupRun(exchanges,  true,  false);

    expect(run.activeIndex).toBe(2);
    expect(run.queuedOrder).toEqual([]);
    expect(run.queuedIndices.size).toBe(0);
  });

  it('still queues a genuine follow-up typed while the latest turn is in flight', () => {
    const exchanges = [
      completedTurn(10, 'first'),
      { userEvent: { type: 'MessageReceived', text: 'working', _eventId: 'm20', created: 't20' } as StoredEvent, userSeq: 20, steps: [ev(21, { type: 'TextStreamed', text: '…' } as StoredEvent)] },
      userMsg(30, 'queued behind it', true),
    ];
    const run = queuedFollowupRun(exchanges, true, false);
    expect(run.activeIndex).toBe(1);
    expect(run.queuedOrder).toEqual([2]);
  });

  it('queues a follow-up typed while parked on a question (latest exchange)', () => {
    const exchanges = [
      completedTurn(10, 'first'),
      answeredQuestionNoTerminal(20),
      userMsg(30, 'typed while question open', true),
    ];
    const run = queuedFollowupRun(exchanges, true, false);
    expect(run.activeIndex).toBe(1);
    expect(run.queuedOrder).toEqual([2]);
  });
});
