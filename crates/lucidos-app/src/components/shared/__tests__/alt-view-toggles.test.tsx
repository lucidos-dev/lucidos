import type { ComponentChildren, VNode } from 'preact';
import { beforeEach, describe, expect, it } from 'vitest';
import { AltViewToggles } from '../AltViewToggles';
import { _resetComposeDraftsForTesting, setDraft } from '../../../store/composeDrafts';
import { attentionViewActive, draftsViewActive, threadMap } from '../../../store/store';
import type { ThreadMeta, ThreadState, ThreadStatus } from '../../../store/thread-events';

type ButtonVNode = VNode<{
  children?: ComponentChildren;
  class?: string;
  disabled?: boolean;
  ['aria-hidden']?: boolean;
  ['aria-label']?: string;
  onClick?: unknown;
}>;

function makeThread(id: string, opts: {
  status?: ThreadStatus;
  state?: ThreadMeta['state'];
  draftText?: string;
} = {}): ThreadState {
  if (opts.draftText !== undefined) {
    setDraft(id, { text: opts.draftText, image_hashes: [], mode: null });
  }
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
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      lastRevivedAt: '',
      state: opts.state ?? 'active',
      latestTodoList: null,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

function buttons(node: ComponentChildren): ButtonVNode[] {
  if (node === null || node === undefined || typeof node === 'boolean' || typeof node === 'string' || typeof node === 'number') return [];
  if (Array.isArray(node)) return node.flatMap(buttons);
  const v = node as VNode<{ children?: ComponentChildren }>;
  if (typeof v.type === 'function') {
    const Fn = v.type as (props: Record<string, unknown>) => ComponentChildren;
    return buttons(Fn(v.props as Record<string, unknown>));
  }
  if (v.type === 'button') return [v as ButtonVNode];
  return buttons(v.props?.children);
}

beforeEach(() => {
  _resetComposeDraftsForTesting();
  attentionViewActive.value = false;
  draftsViewActive.value = false;
  threadMap.value = new Map();
});

describe('AltViewToggles', () => {
  it('keeps both icon slots mounted while inactive', () => {
    const [attention, drafts] = buttons(<AltViewToggles showTooltip />);

    expect(attention.props['aria-label']).toBe('Toggle needs-attention view');
    expect(drafts.props['aria-label']).toBe('Toggle drafts view');
    expect(attention.props.class).toContain('altview-hidden');
    expect(drafts.props.class).toContain('altview-hidden');
    expect(attention.props.disabled).toBe(true);
    expect(drafts.props.disabled).toBe(true);
    expect(attention.props['aria-hidden']).toBe(true);
    expect(drafts.props['aria-hidden']).toBe(true);
    expect(attention.props.onClick).toBeUndefined();
    expect(drafts.props.onClick).toBeUndefined();
  });

  it('only reveals the slot whose count is present', () => {
    const t = makeThread('t1', { draftText: 'Unsent follow-up' });
    threadMap.value = new Map([[t.meta.id, t]]);

    const [attention, drafts] = buttons(<AltViewToggles showTooltip />);

    expect(attention.props.class).toContain('altview-hidden');
    expect(attention.props.disabled).toBe(true);
    expect(drafts.props.class).not.toContain('altview-hidden');
    expect(drafts.props.disabled).toBe(false);
    expect(drafts.props.onClick).toBeTypeOf('function');
  });
});
