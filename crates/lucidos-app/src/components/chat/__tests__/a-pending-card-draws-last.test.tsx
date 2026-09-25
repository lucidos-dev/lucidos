/** A card that reads "Needs your answer" draws last, above only the queued
 *  group, while `exchangeStatus` still reads it at its fold index. See
 *  ADR 0284 and `docs/plans/2026-09-25-pending-card-pins-to-the-bottom.md`.
 */
import { beforeEach, describe, expect, it } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { renderExchanges } from '../CreateThreadView';
import { cancelingThreadIds, removingQueuedMessageIds, threadMap } from '../../../store/store';
import { exchangeStatus, type Exchange, type ThreadState } from '../../../store/thread-events';

const TS = '2026-06-17T12:00:00Z';

function makeThread(status: ThreadState['meta']['status']): ThreadState {
  return {
    meta: {
      id: 't1',
      title: 'Pinned card test',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: TS,
      updatedAt: TS,
      status,
      messageCount: 0,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

const ASKING_TURN: Exchange = {
  userEvent: { type: 'MessageReceived', text: 'release it', created: TS, channel: 'chat', _eventId: 'msg-1' } as any,
  userSeq: 1,
  steps: [{ seq: 2, event: { type: 'ToolCalled', name: 'ask_user_question', args: {}, created: TS } as any }],
};

function question(answered = false): Exchange {
  return {
    userEvent: {
      type: 'UserQuestionAsked',
      tool_use_id: 'q1',
      cc_session_id: '',
      question: 'Wait for it too?',
      options: [{ id: 'a', label: 'Wait' }],
      created: TS,
    } as any,
    userSeq: 3,
    steps: answered
      ? [{ seq: 6, event: { type: 'UserQuestionAnswered', tool_use_id: 'q1', answer: { kind: 'Selected', option_id: 'a' }, created: TS } as any }]
      : [],
  };
}

const CALLBACK: Exchange = {
  userEvent: {
    type: 'ChildThreadCompleted',
    child_thread_id: 'c1',
    status: 'success',
    summary: 'Coding agent stopped working',
    created: TS,
    _eventId: 'ctc-1',
  } as any,
  userSeq: 4,
  steps: [],
};

const QUEUED: Exchange = {
  userEvent: { type: 'MessageReceived', text: 'also this', created: TS, channel: 'chat', _eventId: 'msg-5' } as any,
  userSeq: 5,
  steps: [],
};

function collectExchangeNodes(node: ComponentChildren): VNode<Record<string, unknown>>[] {
  if (node === null || node === undefined || typeof node === 'boolean') return [];
  if (typeof node === 'string' || typeof node === 'number') return [];
  if (Array.isArray(node)) return node.flatMap(collectExchangeNodes);
  const vnode = node as VNode<Record<string, unknown>>;
  const own = 'exchange' in vnode.props && 'isLast' in vnode.props ? [vnode] : [];
  return own.concat(collectExchangeNodes(vnode.props.children as ComponentChildren));
}

function seqs(nodes: VNode[]): number[] {
  return collectExchangeNodes(nodes).map(n => (n.props.exchange as Exchange).userSeq);
}

function statusOf(node: VNode<Record<string, unknown>>): string {
  const p = node.props;
  return exchangeStatus(
    p.exchange as Exchange, '', p.isLast as boolean, p.hasPriorActive as boolean,
    p.threadIsCC as boolean, p.threadIdle as boolean, p.threadAwaitingAnswer as boolean,
  );
}

beforeEach(() => {
  cancelingThreadIds.value = new Set();
  removingQueuedMessageIds.value = new Set();
});

describe('a card awaiting the user draws last', () => {
  it('draws a pending question below a callback that landed after it', () => {
    threadMap.value = new Map([['t1', makeThread('waiting_for_user_answer')]]);
    const nodes = renderExchanges([ASKING_TURN, question(), CALLBACK], 't1', '');
    expect(seqs(nodes)).toEqual([1, 4, 3]);

    // No status moves with the card: the callback keeps its held label.
    const [, callback, card] = collectExchangeNodes(nodes);
    expect(statusOf(callback)).toBe('held');
    expect(statusOf(card)).toBe('awaiting-answer');
  });

  it('keeps the queued group below the pinned card', () => {
    threadMap.value = new Map([['t1', makeThread('waiting_for_user_answer')]]);
    const nodes = renderExchanges([ASKING_TURN, question(), CALLBACK, QUEUED], 't1', '');
    const all = collectExchangeNodes(nodes);
    expect(seqs(nodes)).toEqual([1, 4, 3, 5]);
    expect(all.map(n => n.props.isQueued)).toEqual([false, false, false, true]);
  });

  it('draws the pinned card once even when it sits above the window floor', () => {
    threadMap.value = new Map([['t1', makeThread('waiting_for_user_answer')]]);
    const nodes = renderExchanges([ASKING_TURN, question(), CALLBACK], 't1', '', 2, 3);
    expect(seqs(nodes)).toEqual([4, 3]);
    const card = collectExchangeNodes(nodes)[1];
    expect(card.props.rowsHidden).toBe(0);
  });

  it('leaves an answered question at its fold position', () => {
    threadMap.value = new Map([['t1', makeThread('running')]]);
    const nodes = renderExchanges([ASKING_TURN, question(true), CALLBACK], 't1', '');
    expect(seqs(nodes)).toEqual([1, 3, 4]);
  });

  it('does not pin a card the agent worked past', () => {
    threadMap.value = new Map([['t1', makeThread('running')]]);
    const working: Exchange = {
      ...CALLBACK,
      steps: [{ seq: 5, event: { type: 'CodingAgentTextStreamed', text: 'carrying on', created: TS } as any }],
    };
    const nodes = renderExchanges([ASKING_TURN, question(), working], 't1', '');
    expect(seqs(nodes)).toEqual([1, 3, 4]);
  });

  it('does not pin an unanswered card on a thread that settled without a park', () => {
    threadMap.value = new Map([['t1', makeThread('idle')]]);
    const nodes = renderExchanges([ASKING_TURN, question(), CALLBACK], 't1', '');
    expect(seqs(nodes)).toEqual([1, 3, 4]);
  });
});
