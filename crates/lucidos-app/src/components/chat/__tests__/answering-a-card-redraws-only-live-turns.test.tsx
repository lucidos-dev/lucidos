/** Answering a card flips thread-level flags (`threadIdle` at the tap,
 *  `threadAwaitingAnswer` at the confirmation). A turn those flags cannot change
 *  must keep its memo, or a long thread redraws every row before the pick's
 *  spinner paints. See
 *  `docs/plans/2026-10-24-answering-a-card-redraws-only-live-turns.md`. */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { VNode } from 'preact';
import { chatExchangePropsEqual } from '../ChatExchange';
import { renderExchanges } from '../CreateThreadView';
import { answeringThreadIds, cancelingThreadIds, markThreadAnswering, removingQueuedMessageIds, threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import type { Exchange, StoredEvent } from '../../../store/thread-events';

const TS = '2026-10-24T12:00:00Z';

const finished: Exchange = {
  userEvent: { type: 'MessageReceived', text: 'ship it', created: TS, _eventId: 'msg-1' } as StoredEvent,
  userSeq: 1,
  steps: [
    { seq: 2, event: { type: 'CodingAgentToolCalled', name: 'Bash', args: {}, created: TS } as StoredEvent },
    { seq: 3, event: { type: 'CodingAgentIdled', created: TS } as StoredEvent },
  ],
};

const resumedAfterAnswer: Exchange = {
  userEvent: {
    type: 'UserQuestionAsked',
    tool_use_id: 'q1',
    cc_session_id: '',
    question: 'Wait for it too?',
    options: [{ id: 'a', label: 'Yes' }],
    created: TS,
    _eventId: 'ask-1',
  } as unknown as StoredEvent,
  userSeq: 4,
  steps: [
    { seq: 5, event: { type: 'UserQuestionAnswered', tool_use_id: 'q1', answer: { kind: 'Selected', option_id: 'a' }, created: TS } as unknown as StoredEvent },
    { seq: 6, event: { type: 'CodingAgentToolCalled', name: 'Bash', args: {}, created: TS } as StoredEvent },
  ],
};

const exchanges = [finished, resumedAfterAnswer];

function draw(): VNode[] {
  return renderExchanges(exchanges, 't1', '');
}

beforeEach(() => {
  threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { channel: 'claude_code', status: 'waiting_for_user_answer' } })]]);
  cancelingThreadIds.value = new Set();
  removingQueuedMessageIds.value = new Set();
});

afterEach(() => {
  answeringThreadIds.value = new Set();
  threadMap.value = new Map();
});

describe('tapping an answer on a card', () => {
  it('keeps a finished turn memoised and redraws the turn that resumes', () => {
    const before = draw();
    markThreadAnswering('t1');
    const after = draw();
    expect(after).toHaveLength(2);
    expect(chatExchangePropsEqual(before[0].props as never, after[0].props as never)).toBe(true);
    expect(chatExchangePropsEqual(before[1].props as never, after[1].props as never)).toBe(false);
  });

  it('keeps a finished turn memoised when the confirmation flips the backend status', () => {
    markThreadAnswering('t1');
    const before = draw();
    threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { channel: 'claude_code', status: 'running' } })]]);
    const after = draw();
    expect(chatExchangePropsEqual(before[0].props as never, after[0].props as never)).toBe(true);
  });
});
