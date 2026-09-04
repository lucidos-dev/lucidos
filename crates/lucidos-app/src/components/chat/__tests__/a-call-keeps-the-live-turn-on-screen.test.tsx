/** A caller talking across a working turn takes nothing that is the turn's.
 *
 *  Every utterance is an exchange now, so a call puts many of them between the
 *  running turn and the bottom of the transcript. The turn is no longer the
 *  last exchange, and two things had assumed it was.
 *
 *  The live stream belongs to the turn, wherever it sits. The queued group
 *  belongs at the bottom, where the reader left their unsent messages: anchored
 *  on the turn it would climb above words the caller said after typing them.
 *
 *  See `docs/plans/2026-08-31-a-call-reads-as-one-conversation.md`.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { renderExchanges } from '../CreateThreadView';
import { cancelingThreadIds, removingQueuedMessageIds, threadMap } from '../../../store/store';
import type { Exchange, ThreadState } from '../../../store/thread-events';

const TS = '2026-08-31T07:15:00Z';
const MSG = 'msg-1';

function runningThread(id: string): ThreadState {
  return {
    meta: {
      id,
      title: 'A call',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: TS,
      updatedAt: TS,
      status: 'running',
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

/** The delegated question, mid-turn: it has work under it and no terminator. */
function runningTurn(seq: number): Exchange {
  return {
    userEvent: {
      type: 'MessageReceived',
      text: 'How long will the release take?',
      created: TS,
      channel: 'chat',
      voice_session_id: 'sess-1',
      _eventId: MSG,
    } as never,
    userSeq: seq,
    steps: [{ seq: seq + 1, event: { type: 'ToolCalled', name: 'run_bash', args: {}, created: TS } as never }],
  };
}

/** One thing the caller said, which the talker answered itself. */
function utterance(seq: number, text: string): Exchange {
  return {
    userEvent: { type: 'SpokenMessageReceived', session_id: 'sess-1', text, created: TS } as never,
    userSeq: seq,
    steps: [{ seq: seq + 1, event: { type: 'SpokenReplyGenerated', session_id: 'sess-1', text: 'Still going.', interrupted: false, created: TS } as never }],
  };
}

/** A typed follow-up the reader sent while the doer worked. */
function typed(seq: number, id: string): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'and the changelog?', created: TS, channel: 'chat', _eventId: id } as never,
    userSeq: seq,
    steps: [],
  };
}

function collectVNodes(
  node: ComponentChildren,
  pred: (node: VNode<Record<string, unknown>>) => boolean,
): VNode<Record<string, unknown>>[] {
  if (node === null || node === undefined || typeof node === 'boolean') return [];
  if (typeof node === 'string' || typeof node === 'number') return [];
  if (Array.isArray(node)) return node.flatMap(child => collectVNodes(child, pred));
  const vnode = node as VNode<Record<string, unknown>>;
  const matched = pred(vnode) ? [vnode] : [];
  return matched.concat(collectVNodes(vnode.props.children as ComponentChildren, pred));
}

function exchangeNodes(nodes: VNode[]): VNode<Record<string, unknown>>[] {
  return collectVNodes(nodes, node => 'exchange' in node.props && 'isLast' in node.props);
}

beforeEach(() => {
  threadMap.value = new Map([['t1', runningThread('t1')]]);
  cancelingThreadIds.value = new Set();
  removingQueuedMessageIds.value = new Set();
});

describe('the live turn keeps the stream a caller talks over', () => {
  /** The turn, then six utterances over it. */
  const overTalked = (): Exchange[] => [
    runningTurn(1),
    ...Array.from({ length: 6 }, (_, i) => utterance(10 + i * 2, `Still there? ${i}`)),
  ];

  it('gives the running turn the live stream, and no utterance any of it', () => {
    const nodes = renderExchanges(overTalked(), 't1', 'live tokens');
    const props = exchangeNodes(nodes).map(n => n.props);
    const live = props.filter(p => p.streamingBuffer === 'live tokens');
    expect(live).toHaveLength(1);
    expect((live[0].exchange as Exchange).userSeq).toBe(1);
    expect(live[0].isLast).toBe(true);
  });

  // The window edge is write-once and only moves up (`threadWindow.ts`), so
  // nothing here may widen it. Deriving the floor from the active turn would
  // unmount turns the reader had been shown, the moment the turn settled.
  it('draws exactly the window it was given, whatever the active turn is', () => {
    const nodes = renderExchanges(overTalked(), 't1', 'live tokens', /* renderFromIndex */ 4);
    const drawn = exchangeNodes(nodes).map(n => (n.props.exchange as Exchange).userSeq);
    expect(drawn).toEqual([16, 18, 20]);
  });
});

describe('the queued group rides the bottom of the transcript', () => {
  it('renders below what the caller said after typing it', () => {
    const nodes = renderExchanges(
      [runningTurn(1), typed(5, 'msg-2'), utterance(7, 'Still going?')],
      't1',
      '',
    );
    const order = exchangeNodes(nodes).map(n => (n.props.exchange as Exchange).userSeq);
    expect(order).toEqual([1, 7, 5]);
  });

  it('still sits under the active turn on a typed thread', () => {
    const nodes = renderExchanges([runningTurn(1), typed(5, 'msg-2')], 't1', '');
    const props = exchangeNodes(nodes).map(n => n.props);
    expect(props.map(p => (p.exchange as Exchange).userSeq)).toEqual([1, 5]);
    expect(props.map(p => p.isQueued)).toEqual([false, true]);
  });

  // It draws at the bottom whatever its index, so the window must not judge it
  // by that index. A long call is what separates the two: the message the
  // reader is still waiting on would simply vanish.
  it('draws a message the window floor has left behind', () => {
    const exchanges = [
      runningTurn(1),
      typed(3, 'msg-2'),
      ...Array.from({ length: 6 }, (_, i) => utterance(10 + i * 2, `Still there? ${i}`)),
    ];
    const nodes = renderExchanges(exchanges, 't1', '', /* renderFromIndex */ 5);
    const drawn = exchangeNodes(nodes).map(n => (n.props.exchange as Exchange).userSeq);
    expect(drawn).toContain(3);
    expect(drawn[drawn.length - 1]).toBe(3);
  });
});
