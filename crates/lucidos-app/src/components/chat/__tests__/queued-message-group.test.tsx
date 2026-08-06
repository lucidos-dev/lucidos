import { describe, it, expect, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { renderExchanges } from '../CreateThreadView';
import { cancelingThreadIds, removingQueuedMessageIds, queuedMessageRemovalKey, threadMap } from '../../../store/store';
import type { Exchange, ThreadState } from '../../../store/thread-events';

const TS = '2026-06-17T12:00:00Z';

function makeThread(
  id: string,
  status: ThreadState['meta']['status'] = 'running',
  channel: ThreadState['meta']['channel'] = 'chat',
): ThreadState {
  return {
    meta: {
      id,
      title: 'Queued test',
      channel,
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

function message(text: string, seq: number, steps: Exchange['steps'] = []): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text, created: TS, channel: 'chat', _eventId: `msg-${seq}` } as any,
    userSeq: seq,
    steps,
  };
}

function vnodeText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeText).join('');
  return vnodeText((node as VNode<{ children?: ComponentChildren }>).props.children);
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
  threadMap.value = new Map();
  cancelingThreadIds.value = new Set();
  removingQueuedMessageIds.value = new Set();
});

describe('queued message grouping', () => {
  it('wraps 2+ queued follow-ups in a collapsed Queued group', () => {
    threadMap.value = new Map([['t1', makeThread('t1')]]);
    const active = message('active', 1, [
      { seq: 2, event: { type: 'TextStreamed', text: 'Working...', created: TS } as any },
    ]);
    const nodes = renderExchanges([
      active,
      message('queued one', 3),
      message('queued two', 4),
    ], 't1', 'live tokens');

    const groups = collectVNodes(nodes, node => node.type === 'details' && node.props.class === 'queued-message-group');
    expect(groups).toHaveLength(1);
    expect(groups[0].props.open).toBeUndefined();
    expect(vnodeText(groups[0])).toContain('Queued (2)');

    const props = exchangeNodes(nodes).map(node => node.props);
    expect(props.map(p => p.isQueued)).toEqual([false, true, true]);
    expect(props.map(p => p.streamingBuffer)).toEqual(['live tokens', '', '']);

    expect(props.map(p => (p.exchange as Exchange).userEvent._eventId)).toEqual(['msg-1', 'msg-3', 'msg-4']);
  });

  it('renders one queued follow-up as a plain queued bubble', () => {
    threadMap.value = new Map([['t1', makeThread('t1')]]);
    const active = message('active', 1, [
      { seq: 2, event: { type: 'TextStreamed', text: 'Working...', created: TS } as any },
    ]);
    const nodes = renderExchanges([active, message('queued one', 3)], 't1', 'live tokens');

    const groups = collectVNodes(nodes, node => node.type === 'details' && node.props.class === 'queued-message-group');
    expect(groups).toHaveLength(0);
    expect(exchangeNodes(nodes).map(node => node.props.isQueued)).toEqual([false, true]);
  });

  it('optimistically hides queued messages being removed', () => {
    threadMap.value = new Map([['t1', makeThread('t1')]]);
    removingQueuedMessageIds.value = new Set([queuedMessageRemovalKey('t1', 'msg-3')]);
    const active = message('active', 1, [
      { seq: 2, event: { type: 'TextStreamed', text: 'Working...', created: TS } as any },
    ]);
    const nodes = renderExchanges([
      active,
      message('queued one', 3),
      message('queued two', 4),
    ], 't1', 'live tokens');

    const groups = collectVNodes(nodes, node => node.type === 'details' && node.props.class === 'queued-message-group');
    expect(groups).toHaveLength(0);
    const props = exchangeNodes(nodes).map(node => node.props);
    expect(props.map(p => (p.exchange as Exchange).userSeq)).toEqual([1, 4]);
    expect(props.map(p => p.isQueued)).toEqual([false, true]);
  });

  it('keeps a fresh message after a completed turn active instead of queued', () => {
    threadMap.value = new Map([['t1', makeThread('t1')]]);
    const completed = message('completed', 1, [
      { seq: 2, event: { type: 'ResponseGenerated', created: TS } as any },
    ]);
    const nodes = renderExchanges([completed, message('next turn', 3)], 't1', 'next turn live');

    const props = exchangeNodes(nodes).map(node => node.props);
    expect(props.map(p => p.isQueued)).toEqual([false, false]);
    expect(props.map(p => p.streamingBuffer)).toEqual(['', 'next turn live']);
  });

  it('moves queued messages after a later active question divider', () => {
    threadMap.value = new Map([['t1', makeThread('t1', 'waiting_for_user_answer')]]);
    const active = message('active', 1, [
      { seq: 2, event: { type: 'TextStreamed', text: 'Working...', created: TS } as any },
    ]);
    const question: Exchange = {
      userEvent: {
        type: 'UserQuestionAsked',
        tool_use_id: 'q1',
        cc_session_id: '',
        question: 'Pick one',
        options: [{ id: 'a', label: 'A' }],
      } as any,
      userSeq: 4,
      steps: [],
    };
    const nodes = renderExchanges([active, message('queued before question', 3), question], 't1', '');

    const props = exchangeNodes(nodes).map(node => node.props);
    expect(props.map(p => (p.exchange as Exchange).userSeq)).toEqual([1, 4, 3]);
    expect(props.map(p => p.isQueued)).toEqual([false, false, true]);
    expect(props.map(p => p.isLast)).toEqual([false, true, false]);
  });

  it('does not mark or group CC/Codex follow-ups as queued', () => {
    threadMap.value = new Map([['t1', makeThread('t1', 'running', 'claude_code')]]);
    const nodes = renderExchanges([message('active', 1), message('follow-up', 2)], 't1', 'cc live');

    const groups = collectVNodes(nodes, node => node.type === 'details' && node.props.class === 'queued-message-group');
    expect(groups).toHaveLength(0);
    const props = exchangeNodes(nodes).map(node => node.props);
    expect(props.map(p => p.isQueued)).toEqual([false, false]);
    expect(props.map(p => p.streamingBuffer)).toEqual(['', 'cc live']);
  });
});
