/**
 * The unified thread-drawer Filter dropdown (ThreadFilterDropdown) — one panel
 * with a single-select View section (All / Needs attention / Review / Running /
 * Drafts) on top, a divider, then a multi-select Show (channel) section that is
 * disabled while a non-`all` view is active. These tests invoke the component
 * directly and walk the returned VNode tree WITHOUT descending into the nested
 * function components (<Overlay>, <ExpandableChannelRow>, <TriCheckbox>), which
 * use render-time hooks. The component is hook-free at its own level, so direct
 * invocation is safe.
 *
 * The header button's needs-attention badge renders `attentionThreadCount`
 * directly; its semantics (attention-only — excludes review / running) are
 * covered by `components/drawer/attention-view.test.ts`.
 */
import type { ComponentChildren, VNode } from 'preact';
import { beforeEach, describe, expect, it } from 'vitest';
import { ThreadFilterDropdown, viewIcon } from './ThreadFilterDropdown';
import { drawerView, setDrawerView, threadMap } from '../../store/store';
import { FilterIcon, AttentionIcon, ReviewIcon, RunningIcon, DraftsIcon } from '../shared/icons';
import type { ThreadState, ThreadStatus } from '../../store/thread-events';

type AnyVNode = VNode<Record<string, unknown>>;

function makeThread(id: string, opts: {
  status?: ThreadStatus;
  codingAgentProposed?: boolean;
} = {}): ThreadState {
  return {
    meta: {
      id,
      title: id,
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '2026-05-01T00:00:00Z',
      updatedAt: '2026-05-01T00:00:00Z',
      status: opts.status ?? 'idle',
      messageCount: 1,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      codingAgentHasDiff: false,
      codingAgentProposed: opts.codingAgentProposed ?? false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
    liveEventWaits: [],
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

function asMap(threads: ThreadState[]): Map<string, ThreadState> {
  return new Map(threads.map(t => [t.meta.id, t]));
}

/** Collect DOM (string-typed) vnodes matching `cls`, walking arrays + DOM
 *  children. Deliberately does NOT descend into function components, so the
 *  hooks-using <Overlay> / <ExpandableChannelRow> / <TriCheckbox> are never
 *  invoked. */
function findByClass(node: ComponentChildren, cls: string): AnyVNode[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap(n => findByClass(n, cls));
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return []; // function component — don't invoke
  const out: AnyVNode[] = [];
  const klass = (v.props.class as string | undefined) ?? '';
  if (klass.split(' ').includes(cls)) out.push(v);
  return out.concat(findByClass(v.props.children as ComponentChildren, cls));
}

/** Plain-text content of a vnode subtree (DOM nodes only). */
function textOf(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return '';
  return textOf(v.props.children as ComponentChildren);
}

function render() {
  // The merged dropdown is hook-free at its own level (no useRef/useState at the
  // top), so it can be invoked directly. It returns an <Overlay> whose children
  // hold the View rows, divider, and the Show fieldset.
  const tree = ThreadFilterDropdown({ onClose: () => {}, toggleRef: { current: null } }) as AnyVNode;
  const children = tree.props.children as ComponentChildren;
  const options = findByClass(children, 'drawer-view-option');
  const showFieldset = findByClass(children, 'thread-filter-show')[0];
  return { tree, children, options, showFieldset };
}

beforeEach(() => {
  threadMap.value = new Map();
  setDrawerView('all');
});

describe('ThreadFilterDropdown — View section', () => {
  it('lists the five views in order', () => {
    const { options } = render();
    expect(options.map(o => textOf(o))).toEqual([
      'All statuses', 'Needs attention', 'Review', 'Running', 'Drafts',
    ]);
  });

  it('marks exactly the active view as checked', () => {
    setDrawerView('drafts');
    const checked = render().options.filter(o => o.props['aria-checked'] === true);
    expect(checked).toHaveLength(1);
    expect(textOf(checked[0])).toBe('Drafts');
  });

  it('selecting an option switches the drawer view', () => {
    const review = render().options.find(o => textOf(o) === 'Review')!;
    (review.props.onClick as () => void)();
    expect(drawerView.value).toBe('review');
  });

  it('selecting Running switches the drawer view', () => {
    const running = render().options.find(o => textOf(o) === 'Running')!;
    (running.props.onClick as () => void)();
    expect(drawerView.value).toBe('running');
  });

  it('per-view counts render on the option rows', () => {
    threadMap.value = asMap([
      makeThread('waiting', { status: 'waiting_for_user_answer' }),
      makeThread('proposed', { codingAgentProposed: true }),
      makeThread('running', { status: 'running' }),
    ]);
    const { options } = render();
    const attention = options.find(o => textOf(o).startsWith('Needs attention'))!;
    const review = options.find(o => textOf(o).startsWith('Review'))!;
    const running = options.find(o => textOf(o).startsWith('Running'))!;
    expect(findByClass(attention, 'drawer-view-count').map(textOf)).toEqual(['1']);
    expect(findByClass(review, 'drawer-view-count').map(textOf)).toEqual(['1']);
    expect(findByClass(running, 'drawer-view-count').map(textOf)).toEqual(['1']);
  });

  it('only "Needs attention" wears the blue badge — the others show a plain number', () => {
    threadMap.value = asMap([
      makeThread('waiting', { status: 'waiting_for_user_answer' }),
      makeThread('proposed', { codingAgentProposed: true }),
      makeThread('running', { status: 'running' }),
    ]);
    const { options } = render();
    const attention = options.find(o => textOf(o).startsWith('Needs attention'))!;
    const review = options.find(o => textOf(o).startsWith('Review'))!;
    const running = options.find(o => textOf(o).startsWith('Running'))!;
    // The attention count carries `badge` (blue pill); the others carry only
    // the plain `drawer-view-count` class.
    expect(findByClass(attention, 'badge')).toHaveLength(1);
    expect(findByClass(review, 'badge')).toHaveLength(0);
    expect(findByClass(running, 'badge')).toHaveLength(0);
  });
});

describe('ThreadFilterDropdown — Show (channel) section', () => {
  it('is enabled in the default All view', () => {
    setDrawerView('all');
    expect(render().showFieldset.props.disabled).toBeFalsy();
  });

  it('is disabled (greyed) when a non-All view is active', () => {
    // The alternate views bypass the channel filter, so the whole Show section
    // is disabled in place rather than hidden.
    setDrawerView('review');
    expect(render().showFieldset.props.disabled).toBe(true);
  });
});

describe('viewIcon', () => {
  // The threads-header Filter button reflects the selected view: the funnel for
  // the default `all`, each view's own glyph otherwise.
  it('returns the funnel for all and the view glyph otherwise', () => {
    expect(viewIcon('all')).toBe(FilterIcon);
    expect(viewIcon('attention')).toBe(AttentionIcon);
    expect(viewIcon('review')).toBe(ReviewIcon);
    expect(viewIcon('running')).toBe(RunningIcon);
    expect(viewIcon('drafts')).toBe(DraftsIcon);
  });
});
