import { describe, it, expect, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { renderExchanges } from '../CreateThreadView';
import { renderOriginSection } from '../MessageRoutePanel';
import { cancelingThreadIds, removingQueuedMessageIds, threadMap } from '../../../store/store';
import type { Exchange, StoredEvent, ThreadState } from '../../../store/thread-events';

// A restart the user asked for writes two boundaries: "Paused by restart" and,
// once the new engine is up, "Resumed after engine restart". Read together they
// are one event. So once the resume lands, the pause draws no panel of its own
// and its details move into the resume's info popover.

const TS = '2026-09-24T12:00:00Z';

function makeThread(id: string): ThreadState {
  return {
    meta: {
      id,
      title: 'Restart fold test',
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

const DEVICE = { kind: 'device', device_id: 'd-1', label: 'My iPhone' } as const;

function turn(seq: number): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'do it', created: TS, channel: 'claude_code', _eventId: `u-${seq}` } as StoredEvent,
    userSeq: seq,
    steps: [],
  };
}

function abort(seq: number, fields: Record<string, unknown>, steps: Exchange['steps'] = []): Exchange {
  return {
    userEvent: { type: 'ResponseAborted', created: TS, _eventId: `a-${seq}`, ...fields } as StoredEvent,
    userSeq: seq,
    steps,
  };
}

const pause = (seq: number, steps: Exchange['steps'] = []): Exchange =>
  abort(seq, { cause: 'engine_shutdown', actor: DEVICE }, steps);

function resume(seq: number): Exchange {
  return {
    userEvent: { type: 'ContinuationStarted', reason: 'auto_resume_after_switch', created: TS, _eventId: `c-${seq}` } as StoredEvent,
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

function render(exchanges: Exchange[]) {
  return exchangeNodes(renderExchanges(exchanges, 't1', ''));
}

const types = (nodes: VNode<Record<string, unknown>>[]) =>
  nodes.map(n => (n.props.exchange as Exchange).userEvent.type);

beforeEach(() => {
  threadMap.value = new Map([['t1', makeThread('t1')]]);
  cancelingThreadIds.value = new Set();
  removingQueuedMessageIds.value = new Set();
});

describe('a restart pause folds into the resume that answers it', () => {
  it('draws no pause panel once the resume lands, and hands the pause to the resume', () => {
    const exchanges = [turn(1), pause(2), resume(3)];
    const nodes = render(exchanges);
    expect(types(nodes)).toEqual(['MessageReceived', 'ContinuationStarted']);
    expect(nodes[1].props.pausedBy).toBe(exchanges[1].userEvent);
  });

  it('keeps the pause panel while the engine is still coming back', () => {
    const nodes = render([turn(1), pause(2)]);
    expect(types(nodes)).toEqual(['MessageReceived', 'ResponseAborted']);
  });

  it('keeps the pause panel when something else sits between it and the resume', () => {
    const nodes = render([turn(1), pause(2), turn(3), resume(4)]);
    expect(types(nodes)).toEqual(['MessageReceived', 'ResponseAborted', 'MessageReceived', 'ContinuationStarted']);
    expect(nodes[3].props.pausedBy).toBeUndefined();
  });

  it('keeps a crash interruption visible: only a restart the user asked for folds', () => {
    const nodes = render([turn(1), abort(2, { cause: 'process_killed', actor: { kind: 'system' } }), resume(3)]);
    expect(types(nodes)).toEqual(['MessageReceived', 'ResponseAborted', 'ContinuationStarted']);
    expect(nodes[2].props.pausedBy).toBeUndefined();
  });

  it('keeps a pause that holds work of its own', () => {
    const work: Exchange['steps'] = [{
      seq: 2.5,
      event: { type: 'CodingAgentToolCalled', name: 'Bash', tool_use_id: 'tu-1', args: { command: 'ls' }, created: TS } as unknown as StoredEvent,
    }];
    const nodes = render([turn(1), pause(2, work), resume(3)]);
    expect(types(nodes)).toContain('ResponseAborted');
  });
});

describe('the resume popover carries the folded pause', () => {
  it('says why it paused and which device restarted', () => {
    const exchanges = [pause(2), resume(3)];
    const s = JSON.stringify(renderOriginSection(exchanges[1], undefined, () => undefined, exchanges[0].userEvent));
    expect(s).toContain('Why this resumed');
    expect(s).toContain('Why it paused');
    expect(s).toContain('Restarted from');
    expect(s).toContain('My iPhone');
  });

  it('adds nothing when no pause was folded in', () => {
    const s = JSON.stringify(renderOriginSection(resume(3), undefined, () => undefined));
    expect(s).not.toContain('Why it paused');
  });
});
