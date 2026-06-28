import { describe, it, expect, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { renderExchanges } from '../CreateThreadView';
import { cancelingThreadIds, removingQueuedMessageIds, threadMap } from '../../../store/store';
import type { Exchange, ThreadState } from '../../../store/thread-events';

// Regression: opening a thread that ends with "Change applied" used to jump on
// FIRST load. The ChangeApplied card's body + footer render only once the per-id
// `Change` row is fetched, so on first open they popped in late and the open-path
// scroll-to-bottom re-pinned to the grown bottom (everything shifted up). The fix
// seeds the body from the in-thread `ChangeProposed` (already loaded with the
// thread) so the card paints at its final height immediately. This pins the
// wiring: renderExchanges harvests the proposed description + file count and
// hands them to the matching ChangeApplied exchange's ChatExchange node.

const TS = '2026-06-17T12:00:00Z';

function makeThread(id: string): ThreadState {
  return {
    meta: {
      id,
      title: 'Change seed test',
      channel: 'claude_code',
      initiator: 'user',
      saved: false,
      createdAt: TS,
      updatedAt: TS,
      status: 'idle',
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
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

/** A coding-agent turn that proposed `changeId`, carrying the ChangeProposed as
 *  a (non-rendered) step — the only place the description + file list live. */
function ccTurn(seq: number, step: Record<string, unknown>): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'do it', created: TS, channel: 'claude_code', _eventId: `u-${seq}` } as any,
    userSeq: seq,
    steps: [{ seq: seq + 0.5, event: { type: 'ChangeProposed', created: TS, ...step } as any }],
  };
}

function changeApplied(seq: number, changeId: string): Exchange {
  return {
    userEvent: { type: 'ChangeApplied', change_id: changeId, created: TS, _eventId: `a-${seq}` } as any,
    userSeq: seq,
    steps: [],
  };
}

function exchangeNodes(node: ComponentChildren): VNode<Record<string, unknown>>[] {
  if (node === null || node === undefined || typeof node === 'boolean') return [];
  if (typeof node === 'string' || typeof node === 'number') return [];
  if (Array.isArray(node)) return node.flatMap(exchangeNodes);
  const vnode = node as VNode<Record<string, unknown>>;
  const matched = ('exchange' in vnode.props && 'isLast' in vnode.props) ? [vnode] : [];
  return matched.concat(exchangeNodes(vnode.props.children as ComponentChildren));
}

function appliedNode(nodes: VNode[], changeId: string): VNode<Record<string, unknown>> | undefined {
  return exchangeNodes(nodes).find(n => {
    const ev = (n.props.exchange as Exchange).userEvent as { type: string; change_id?: string };
    return ev.type === 'ChangeApplied' && ev.change_id === changeId;
  });
}

beforeEach(() => {
  threadMap.value = new Map();
  cancelingThreadIds.value = new Set();
  removingQueuedMessageIds.value = new Set();
});

describe('change-applied body seeding (open-jump fix)', () => {
  it('seeds the applied card with the in-thread ChangeProposed description + file count', () => {
    threadMap.value = new Map([['t1', makeThread('t1')]]);
    const nodes = renderExchanges([
      ccTurn(1, { change_id: 'c1', description: 'Fix the bug\nbody line', files: ['a.ts', 'b.ts', 'c.ts'] }),
      changeApplied(2, 'c1'),
    ], 't1', '');

    const applied = appliedNode(nodes, 'c1');
    // Full description is forwarded; ChangeBody clamps to the first line itself.
    expect(applied?.props.proposedChangeDesc).toBe('Fix the bug\nbody line');
    expect(applied?.props.proposedChangeFileCount).toBe(3);
  });

  it('leaves the seed undefined when no matching ChangeProposed rode the thread', () => {
    threadMap.value = new Map([['t1', makeThread('t1')]]);
    // ChangeApplied for c2, but the only proposal in-thread was for c1.
    const nodes = renderExchanges([
      ccTurn(1, { change_id: 'c1', description: 'unrelated', files: ['x.ts'] }),
      changeApplied(2, 'c2'),
    ], 't1', '');

    const applied = appliedNode(nodes, 'c2');
    expect(applied?.props.proposedChangeDesc).toBeUndefined();
    expect(applied?.props.proposedChangeFileCount).toBeUndefined();
  });

  it('ignores per-commit ChangeProposed emits with an empty change_id', () => {
    threadMap.value = new Map([['t1', makeThread('t1')]]);
    const nodes = renderExchanges([
      // A per-commit emit (empty change_id) precedes the aggregate proposal.
      ccTurn(1, { change_id: '', description: 'per-commit noise', files: ['noise.ts'] }),
      ccTurn(3, { change_id: 'c1', description: 'real summary', files: ['a.ts', 'b.ts'] }),
      changeApplied(4, 'c1'),
    ], 't1', '');

    const applied = appliedNode(nodes, 'c1');
    expect(applied?.props.proposedChangeDesc).toBe('real summary');
    expect(applied?.props.proposedChangeFileCount).toBe(2);
  });
});
