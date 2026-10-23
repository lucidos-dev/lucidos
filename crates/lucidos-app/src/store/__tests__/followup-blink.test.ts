// Regression for the follow-up "blink": send a follow-up to an IDLE chat
// thread and the just-sent optimistic bubble flickers — it briefly shows at
// the bottom, then jumps into a "Queued" group UP in history.
//
// Root cause: `queuedFollowupRun` looks for the turn that owns queued
// follow-ups by walking from the end for the first exchange that
// `canQueueBehind`. On a thread whose history contains an earlier answered
// `UserQuestionAsked` whose continuation flowed into the NEXT question without
// ever producing a terminal step, that stale divider reads as still-queuable.
// The search walked PAST the most recent (terminal) turn and latched onto it,
// rendering the fresh follow-up as "queued" behind a question high up in the
// transcript. The thread is idle → the follow-up IS the active turn and must
// render at the bottom.
//
// The fix: the active turn (if any) is the MOST RECENT non-uningested
// exchange. If it's terminal, the thread idled and the send is active — don't
// walk into older non-terminal turns.

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

/** An answered question divider whose continuation never produced a terminal
 *  step (the turn flowed straight into the next question). */
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

/** A completed turn (ends in ResponseGenerated). */
function completedTurn(seq: number, text: string): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text, _eventId: `m${seq}`, created: `t${seq}` } as StoredEvent,
    userSeq: seq,
    steps: [ev(seq + 1, { type: 'ResponseGenerated', text: 'ok' } as StoredEvent)],
  };
}

describe('follow-up to an idle thread is the active turn, never queued in history', () => {
  it('does not queue the follow-up behind a stale answered-question divider', () => {
    // Mirrors real thread aa75ff37: an early answered question that never
    // terminated, then a fully-completed turn, then the fresh follow-up.
    const exchanges = [
      answeredQuestionNoTerminal(10), // stale non-terminal divider, up in history
      completedTurn(20, 'second'), // most recent real turn — terminal
      userMsg(30, 'my follow-up', true), // just-sent, now persisted; thread running
    ];
    const run = queuedFollowupRun(exchanges, /* threadBusy */ true, /* threadIsCC */ false);

    // The follow-up (last index) is the active turn and nothing is queued.
    expect(run.activeIndex).toBe(2);
    expect(run.queuedOrder).toEqual([]);
    expect(run.queuedIndices.size).toBe(0);
  });

  it('still queues a genuine follow-up typed while the latest turn is in flight', () => {
    const exchanges = [
      completedTurn(10, 'first'),
      // latest turn still streaming (no terminal step)
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
      answeredQuestionNoTerminal(20), // most recent: an in-flight question divider
      userMsg(30, 'typed while question open', true),
    ];
    const run = queuedFollowupRun(exchanges, true, false);
    expect(run.activeIndex).toBe(1);
    expect(run.queuedOrder).toEqual([2]);
  });
});
