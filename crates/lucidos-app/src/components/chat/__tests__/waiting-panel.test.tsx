import { afterEach, describe, it, expect, vi } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import {
  awaitedSubThreads,
  SubThreadRow,
  subscriptionLine,
  waitingIndicatorBody,
  waitingPanelBody,
  waitingPanelCondition,
  type SubThreadWait,
} from '../WaitingPanel';
import { eventConditionPopover } from '../../../store/store';
import { SurfaceHead } from '../../shared/Surface';

const focusThreadOrBootstrap = vi.fn();
vi.mock('../../../store/actions/threads', () => ({
  focusThreadOrBootstrap: (id: string) => focusThreadOrBootstrap(id),
}));
import type { EventWaitSummary, ThreadMeta, ThreadState, ThreadStatus } from '../../../store/thread-events';

function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<Record<string, unknown> & { children?: ComponentChildren }>;
  if (v.type === SurfaceHead) {
    return vnodeToText(SurfaceHead(v.props as Parameters<typeof SurfaceHead>[0]));
  }
  const tag = typeof v.type === 'string' ? v.type : '';
  const attrs: string[] = [];
  for (const [k, val] of Object.entries(v.props ?? {})) {
    if (k === 'children') continue;
    if (k.startsWith('on')) continue;
    if (val === undefined || val === null || val === false) continue;
    attrs.push(` ${k}="${val}"`);
  }
  const inner = vnodeToText(v.props?.children);
  return tag ? `<${tag}${attrs.join('')}>${inner}</${tag}>` : inner;
}

const NOOP = () => {};
const NO_SUB_THREADS: SubThreadWait = { threads: [], unresolved: 0 };

function wait(over: Partial<EventWaitSummary> = {}): EventWaitSummary {
  return {
    wait_id: 'w1',
    reason: 'Waiting for the two remaining coding-agent threads to land',
    on: [{ event_type: 'ChangeProposed' }, { event_type: 'CodingAgentIdled' }],
    expires_at: new Date(Date.now() + 3_600_000).toISOString(),
    ...over,
  } as EventWaitSummary;
}

function thread(id: string, over: Partial<ThreadMeta> = {}): ThreadState {
  return {
    meta: {
      id,
      title: `Thread ${id}`,
      channel: 'claude_code',
      createdAt: `2026-08-22T10:00:0${id.length}Z`,
      updatedAt: '2026-08-22T10:00:00Z',
      status: 'running' as ThreadStatus,
      messageCount: 1,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      liveEventWaitCount: 0,
      liveEventWaits: [],
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
      saved: false,
      ...over,
    } as ThreadMeta,
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  } as ThreadState;
}

function map(...threads: ThreadState[]): Map<string, ThreadState> {
  return new Map(threads.map((t) => [t.meta.id, t]));
}


describe('awaitedSubThreads', () => {
  it('names nothing while the server counts no awaited child', () => {
    const children = map(
      thread('c', { parentThreadId: 'p', status: 'running', awaitedByParent: true }),
      thread('no-wake', { parentThreadId: 'p', status: 'running' }),
    );
    expect(awaitedSubThreads('p', children, 0)).toEqual({ threads: [], unresolved: 0 });
  });

  it('takes the children the engine flags as awaited, and no others', () => {
    const children = map(
      thread('running', { parentThreadId: 'p', status: 'running', awaitedByParent: true }),
      thread('awaiting', { parentThreadId: 'p', status: 'idle', awaitedByParent: true }),
      thread('no-wake', { parentThreadId: 'p', status: 'running' }),
      thread('finished', { parentThreadId: 'p', status: 'idle' }),
      thread('elsewhere', { parentThreadId: 'other', status: 'running', awaitedByParent: true }),
    );
    const found = awaitedSubThreads('p', children, 2);
    expect(found.threads.map((t) => t.meta.id).sort()).toEqual(['awaiting', 'running']);
    expect(found.unresolved).toBe(0);
  });

  it('lists an awaited idle child when the count is 1', () => {
    const children = map(thread('c1', { parentThreadId: 'p', status: 'idle', awaitedByParent: true }));
    expect(awaitedSubThreads('p', children, 1)).toEqual({ threads: [children.get('c1')], unresolved: 0 });
  });

  it('reports the children the server counts and the map cannot name', () => {
    const children = map(thread('c1', { parentThreadId: 'p', status: 'running', awaitedByParent: true }));
    const found = awaitedSubThreads('p', children, 3);
    expect(found.threads.map((t) => t.meta.id)).toEqual(['c1']);
    expect(found.unresolved).toBe(2);
  });

  it('never reports a negative shortfall when the map is ahead of the count', () => {
    const children = map(
      thread('c1', { parentThreadId: 'p', status: 'running', awaitedByParent: true }),
      thread('c2', { parentThreadId: 'p', status: 'running', awaitedByParent: true }),
    );
    expect(awaitedSubThreads('p', children, 1).unresolved).toBe(0);
  });
});


describe('waitingIndicatorBody', () => {
  it('renders nothing when the thread waits on nothing', () => {
    expect(
      waitingIndicatorBody({ waits: [], subThreads: NO_SUB_THREADS, onClick: NOOP }),
    ).toBeNull();
  });

  it('puts the single wait reason in the tooltip, and a count when there are several', () => {
    const one = vnodeToText(
      waitingIndicatorBody({
        waits: [wait({ reason: 'until v0.23.0' })],
        subThreads: NO_SUB_THREADS,
        onClick: NOOP,
      }),
    );
    expect(one).toContain('data-tooltip="until v0.23.0"');
    const many = vnodeToText(
      waitingIndicatorBody({
        waits: [wait(), wait({ wait_id: 'w2' })],
        subThreads: NO_SUB_THREADS,
        onClick: NOOP,
      }),
    );
    expect(many).toContain('data-tooltip="2 events"');
  });

  it('renders for sub-threads alone, naming them rather than a subscription', () => {
    const text = vnodeToText(
      waitingIndicatorBody({
        waits: [],
        subThreads: { threads: [thread('c1'), thread('c2')], unresolved: 0 },
        onClick: NOOP,
      }),
    );
    expect(text).toContain('data-role="waiting-indicator"');
    expect(text).toContain('data-tooltip="2 sub-threads"');
  });

  it('counts the children the map cannot name', () => {
    const text = vnodeToText(
      waitingIndicatorBody({
        waits: [],
        subThreads: { threads: [thread('c1')], unresolved: 2 },
        onClick: NOOP,
      }),
    );
    expect(text).toContain('data-tooltip="3 sub-threads"');
  });

  it('speaks the reason once, however the model phrased it', () => {
    const text = vnodeToText(
      waitingIndicatorBody({
        waits: [wait({ reason: 'waiting for the release build' })],
        subThreads: NO_SUB_THREADS,
        onClick: NOOP,
      }),
    );
    expect(text).toContain('data-tooltip="waiting for the release build"');
    expect(text).toContain('aria-label="Waiting for the release build. Click to expand."');
  });

  it('names both kinds when the thread is parked on both', () => {
    const text = vnodeToText(
      waitingIndicatorBody({
        waits: [wait({ reason: 'until the release lands' })],
        subThreads: { threads: [thread('c1')], unresolved: 0 },
        onClick: NOOP,
      }),
    );
    expect(text).toContain('data-tooltip="1 event, 1 sub-thread"');
    expect(text).toContain('aria-label="Waiting for 1 event, 1 sub-thread. Click to expand."');
  });
});


describe('waitingPanelBody', () => {
  const body = (over: Partial<Parameters<typeof waitingPanelBody>[0]> = {}) =>
    vnodeToText(
      waitingPanelBody({
        threadId: 't1',
        waits: [wait()],
        subThreads: NO_SUB_THREADS,
        onClose: NOOP,
        ...over,
      }),
    );

  it('gives the close button its own header strip, above the list', () => {
    const text = body();
    const head = text.indexOf('surface-head');
    const close = text.indexOf('surface-close');
    const list = text.indexOf('event-wait-list');
    expect(head).toBeGreaterThanOrEqual(0);
    expect(close).toBeGreaterThan(head);
    expect(list).toBeGreaterThan(close);
    expect(text).toContain('aria-label="Close what this thread is waiting for"');
    expect(text).toContain('>Waiting for<');
  });

  it('renders the list inside the padded body, not directly on the shell', () => {
    expect(body()).toContain('<div data-surface-step="list" class="anchored-popover-body">');
    expect(body()).toContain('<ul class="event-wait-list">');
  });

  it('renders one header strip however many waits are live', () => {
    const text = body({ waits: [wait({ wait_id: 'a' }), wait({ wait_id: 'b' })] });
    expect(text).toContain('event-wait-list');
    expect((text.match(/class="surface-head"/g) ?? []).length).toBe(1);
  });

  it('omits a section it has no rows for', () => {
    const subsOnly = body();
    expect(subsOnly).toContain('data-role="waiting-subscriptions"');
    expect(subsOnly).not.toContain('data-role="waiting-sub-threads"');
    const childrenOnly = body({ waits: [], subThreads: { threads: [thread('c1')], unresolved: 0 } });
    expect(childrenOnly).toContain('data-role="waiting-sub-threads"');
    expect(childrenOnly).not.toContain('data-role="waiting-subscriptions"');
  });

  it('labels both sections when the thread is parked on both', () => {
    const text = body({ subThreads: { threads: [thread('c1')], unresolved: 0 } });
    expect(text).toContain('>Events<');
    expect(text).toContain('>Sub-threads<');
  });

  it('leaves a lone section unlabelled', () => {
    expect(body()).not.toContain('waiting-panel-section-label');
    const childrenOnly = body({ waits: [], subThreads: { threads: [thread('c1')], unresolved: 0 } });
    expect(childrenOnly).not.toContain('waiting-panel-section-label');
  });

  it('drills into a condition in place, with a way back to the list', () => {
    const text = body({ condition: { eventType: 'CodingAgentIdled', conditions: [{ thread_id: 't1' }] } });
    expect(text).toContain('data-role="surface-back"');
    expect(text).toContain('Waiting for');
    expect(text).toContain('>Condition<');
    expect(text).toContain('data-role="waiting-condition"');
    expect(text).toContain('data-role="event-condition-json"');
    expect(text).not.toContain('event-wait-list');
    expect(text).toContain('aria-label="Close what this thread is waiting for"');
  });

  it('steps back to the list from the drill-in', () => {
    const onBack = vi.fn();
    const tree = waitingPanelBody({
      threadId: 't1',
      waits: [wait()],
      subThreads: NO_SUB_THREADS,
      condition: { eventType: 'CodingAgentIdled', conditions: [{ thread_id: 't1' }] },
      onBack,
      onClose: NOOP,
    }) as VNode<{ children: ComponentChildren }>;
    const head = (tree.props.children as VNode<{ back: { onClick: () => void } }>[])[0];
    head.props.back.onClick();
    expect(onBack).toHaveBeenCalledOnce();
  });

  it('says how many children it could not name, instead of a short list', () => {
    const text = body({ waits: [], subThreads: { threads: [thread('c1')], unresolved: 2 } });
    expect(text).toContain('data-role="waiting-sub-threads-more"');
    expect(text).toContain('and 2 more');
  });
});

describe('the panel subscription line', () => {
  afterEach(() => {
    eventConditionPopover.value = null;
    waitingPanelCondition.value = null;
  });

  const CONDITION = { 'workflow_run.event': 'completed' };

  it('reads as one joined line whether or not an entry is with a condition', () => {
    const text = vnodeToText(subscriptionLine([
      { event_type: 'GithubWorkflowRunStateChanged', condition: CONDITION },
      { event_type: 'ChangeProposed' },
    ]));
    expect(text).toContain('watching for ');
    expect(text).toContain('github workflow run state changed (with a condition)');
    expect(text).toContain(' or ');
    expect(text).toContain('change proposed');
  });

  it('opens the condition from a filtered entry', () => {
    const line = subscriptionLine([
      { event_type: 'GithubWorkflowRunStateChanged', condition: CONDITION },
    ]);
    const button = line[1] as VNode<{ 'aria-label': string; onClick: () => void }>;
    expect(button.type).toBe('button');
    expect(button.props['aria-label']).toBe('github workflow run state changed · show the condition');
    button.props.onClick();
    expect(waitingPanelCondition.value).toEqual({
      eventType: 'GithubWorkflowRunStateChanged',
      conditions: [CONDITION],
    });
    expect(eventConditionPopover.value).toBeNull();
  });

  it('leaves an unfiltered entry as plain text', () => {
    const line = subscriptionLine([{ event_type: 'ChangeProposed' }]);
    expect((line[1] as VNode).type).toBe('span');
    expect(vnodeToText(line)).toBe(
      '<span>watching for </span><span data-tooltip="An agent offered a change for you to apply.">change proposed</span>',
    );
  });
});

describe('a sub-thread row', () => {
  function clickRow(onOpen: () => void): void {
    const li = SubThreadRow({ child: thread('c1'), onOpen }) as VNode<{
      children?: ComponentChildren;
    }>;
    const button = li.props.children as VNode<{ onClick: () => void }>;
    button.props.onClick();
  }

  it('opens the child thread', () => {
    focusThreadOrBootstrap.mockClear();
    clickRow(NOOP);
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('c1');
  });

  it('closes the panel on the way out', () => {
    const onOpen = vi.fn();
    clickRow(onOpen);
    expect(onOpen).toHaveBeenCalled();
  });
});
