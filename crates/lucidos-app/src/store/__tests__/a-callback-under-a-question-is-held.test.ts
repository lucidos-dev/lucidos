/**
 * A callback that arrives while the user owes the agent an answer waits behind
 * that answer (ADR 0255). Its card says so. "Requesting" claimed the agent was
 * already on it. See `docs/plans/2026-09-24-a-delivery-never-unparks-a-question.md`.
 */
import { describe, it, expect } from 'vitest';
import {
  exchangeStatus,
  groupIntoExchanges,
  type Exchange,
  type StoredEvent,
  type ThreadEvent,
} from '../thread-events';
import { statusLabel } from '../exchange-status';

function ev(seq: number, e: ThreadEvent, id?: string): readonly [number, StoredEvent] {
  const created = `2026-09-24T18:49:${String(seq).padStart(2, '0')}Z`;
  return [seq, { ...e, created, ...(id ? { _eventId: id } : {}) } as StoredEvent] as const;
}

const QUESTION: ThreadEvent = {
  type: 'UserQuestionAsked',
  tool_use_id: 'tu1',
  cc_session_id: '',
  question: 'How should I handle the proxy timeout session?',
  options: [{ id: 'opt-0', label: 'Build move-to-top-level' }],
};

const CHILD_RETURNED = {
  type: 'ChildThreadCompleted', child_thread_id: 'c1', status: 'success', summary: 'done',
} as ThreadEvent;

const DELIVERY_ANCHOR = {
  type: 'UserPromptInjected', text: 'An event you subscribed to has arrived', mode: 'agent',
} as ThreadEvent;

/** Status as the transcript renders it while the thread is parked on the
 *  question: `waiting_for_user_answer` is quiescent AND awaiting an answer. */
function parkedStatus(exchanges: Exchange[], index: number, threadIsCC = false) {
  const isLast = index === exchanges.length - 1;
  return exchangeStatus(exchanges[index], '', isLast, false, threadIsCC, true, true);
}

describe('a callback under an open question', () => {
  const exchanges = groupIntoExchanges(new Map([
    ev(1, { type: 'MessageReceived', text: 'run the loop' }),
    ev(2, QUESTION),
    ev(3, CHILD_RETURNED),
    ev(4, DELIVERY_ANCHOR, 'anchor-1'),
  ]));
  const last = exchanges.length - 1;

  it('reads "Held until you reply", never "Requesting"', () => {
    expect(exchanges[last].userEvent.type).toBe('UserPromptInjected');
    expect(parkedStatus(exchanges, last)).toBe('held');
    expect(statusLabel('held', false).label).toBe('Held until you reply');
  });

  it('leaves the question itself asking for the answer', () => {
    const divider = exchanges.findIndex(e => e.userEvent.type === 'UserQuestionAsked');
    expect(parkedStatus(exchanges, divider)).toBe('awaiting-answer');
  });

  it('holds a returned child the same way when it is the newest card', () => {
    const childLast = groupIntoExchanges(new Map([
      ev(1, { type: 'MessageReceived', text: 'run the loop' }),
      ev(2, QUESTION),
      ev(3, CHILD_RETURNED),
    ]));
    expect(parkedStatus(childLast, childLast.length - 1)).toBe('held');
  });

  it('holds on a coding-agent thread too', () => {
    expect(parkedStatus(exchanges, last, /* threadIsCC */ true)).toBe('held');
  });

  it('reads "Requesting" as soon as the user answers', () => {
    // The answer is in, but the engine's `running` has not reached the client.
    const answered = exchangeStatus(exchanges[last], '', true, false, false, false, true);
    expect(answered).toBe('pending');
    const resumed = exchangeStatus(exchanges[last], '', true, false, false, false, false);
    expect(resumed).toBe('pending');
  });
});
