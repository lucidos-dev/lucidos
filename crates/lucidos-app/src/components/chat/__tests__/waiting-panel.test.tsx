import { describe, it, expect, vi } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import {
  activeSubThreads,
  SubThreadRow,
  waitingIndicatorBody,
  waitingPanelBody,
  type SubThreadWait,
} from '../WaitingPanel';

const focusThreadOrBootstrap = vi.fn();
vi.mock('../../../store/actions/threads', () => ({
  focusThreadOrBootstrap: (id: string) => focusThreadOrBootstrap(id),
}));
import type { EventWaitSummary, ThreadMeta, ThreadState, ThreadStatus } from '../../../store/thread-events';

/** Flatten a vnode tree into HTML-ish text preserving class / data-* / aria-*
 *  attributes. Same helper as `todo-list-panel.test.tsx` beside it. */
function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<Record<string, unknown> & { children?: ComponentChildren }>;
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
      codingAgentHasDiff: false,
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
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

// ──────────────────────────────────────────────────────────────────────────
// Which children count, and who decides how many there are.
// ──────────────────────────────────────────────────────────────────────────

describe('activeSubThreads', () => {
  it('names nothing while the server counts no active child', () => {
    const children = map(thread('c', { parentThreadId: 'p', status: 'running' }));
    expect(activeSubThreads('p', children, 0)).toEqual({ threads: [], unresolved: 0 });
  });

  it('takes the running and question children, and no others', () => {
    const children = map(
      thread('running', { parentThreadId: 'p', status: 'running' }),
      thread('asking', { parentThreadId: 'p', status: 'waiting_for_user_answer' }),
      // Idle while holding its own subscription, and idle with a proposed
      // change: both are waiting for somebody, neither is mid-turn, and the
      // engine's `active_thread_statuses()` excludes both.
      thread('subscribed', { parentThreadId: 'p', status: 'idle', liveEventWaitCount: 1 }),
      thread('proposed', { parentThreadId: 'p', status: 'waiting', codingAgentProposed: true }),
      thread('elsewhere', { parentThreadId: 'other', status: 'running' }),
    );
    const found = activeSubThreads('p', children, 2);
    expect(found.threads.map((t) => t.meta.id).sort()).toEqual(['asking', 'running']);
    expect(found.unresolved).toBe(0);
  });

  it('reports the children the server counts and the map cannot name', () => {
    const children = map(thread('c1', { parentThreadId: 'p', status: 'running' }));
    const found = activeSubThreads('p', children, 3);
    expect(found.threads.map((t) => t.meta.id)).toEqual(['c1']);
    expect(found.unresolved).toBe(2);
  });

  it('never reports a negative shortfall when the map is ahead of the count', () => {
    const children = map(
      thread('c1', { parentThreadId: 'p', status: 'running' }),
      thread('c2', { parentThreadId: 'p', status: 'running' }),
    );
    expect(activeSubThreads('p', children, 1).unresolved).toBe(0);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// The button: it exists exactly while the thread is parked on something.
// ──────────────────────────────────────────────────────────────────────────

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
    expect(many).toContain('data-tooltip="2 subscriptions"');
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

  /** The tooltip says the model's words as written, and carries no verb of its
   *  own. The aria-label does carry one, so it takes the subject instead: every
   *  reason for this concept contains a waiting word, and "Waiting for waiting
   *  for X" is the doubled label
   *  `docs/plans/2026-08-14-a-wait-label-does-not-say-waiting-twice.md` exists
   *  to prevent. */
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
    // The lone-reason shortcut is off here: two reasons do not fit a tooltip.
    expect(text).toContain('data-tooltip="1 subscription, 1 sub-thread"');
    expect(text).toContain('aria-label="Waiting for 1 subscription, 1 sub-thread. Click to expand."');
  });
});

// ──────────────────────────────────────────────────────────────────────────
// Panel: the close button lives in the shell's header strip, never floating
// over the rows. It was absolutely positioned in the panel's top-right corner,
// where it sat on top of the description as soon as that description wrapped.
// Being absolute inside the scrolling box, it also scrolled away with a long
// list. The structure is what fixes that, so the structure is what this pins.
// ──────────────────────────────────────────────────────────────────────────

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
    const head = text.indexOf('prompt-bar-popover-head');
    const close = text.indexOf('prompt-bar-popover-close');
    const list = text.indexOf('event-wait-list');
    expect(head).toBeGreaterThanOrEqual(0);
    expect(close).toBeGreaterThan(head);
    expect(list).toBeGreaterThan(close);
    expect(text).toContain('aria-label="Close what this thread is waiting for"');
    expect(text).toContain('>Waiting for<');
  });

  it('renders the list inside the padded body, not directly on the shell', () => {
    expect(body()).toContain('<div class="prompt-bar-popover-body">');
    expect(body()).toContain('<ul class="event-wait-list">');
  });

  it('renders one header strip however many waits are live', () => {
    const text = body({ waits: [wait({ wait_id: 'a' }), wait({ wait_id: 'b' })] });
    // Rows are their own component, so they render as an empty vnode here;
    // the list container plus the head is what this body owns.
    expect(text).toContain('event-wait-list');
    expect((text.match(/prompt-bar-popover-head/g) ?? []).length).toBe(1);
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
    expect(text).toContain('>Subscriptions<');
    expect(text).toContain('>Sub-threads<');
  });

  it('says how many children it could not name, instead of a short list', () => {
    const text = body({ waits: [], subThreads: { threads: [thread('c1')], unresolved: 2 } });
    expect(text).toContain('data-role="waiting-sub-threads-more"');
    expect(text).toContain('and 2 more');
  });
});

describe('a sub-thread row', () => {
  /** The click handler off the row's button, which the vnode walkers above
   *  deliberately drop (they render attributes, not behaviour). */
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

  /** The panel describes the thread it was opened from, so it must not survive
   *  a jump to another one: left open it would re-read against the child. */
  it('closes the panel on the way out', () => {
    const onOpen = vi.fn();
    clickRow(onOpen);
    expect(onOpen).toHaveBeenCalled();
  });
});
