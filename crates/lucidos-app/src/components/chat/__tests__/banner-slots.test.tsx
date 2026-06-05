import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { getBannerSlots, getWaitingState, getStandaloneCcDiffButton } from '../WaitingBanner';
import {
  threadMap,
  focusedThreadId,
  archivingThreadIds,
  applyingNowThreadIds,
  discardingCCThreadIds,
  cancelingThreadIds,
  changes,
} from '../../../store/store';
import type { ThreadState } from '../../../store/thread-events';
import type { TaggedAction } from '../../../store/actions/threadActions';

// Stub repositories actions so we can assert which one the Diff button click
// calls. Importing the real module would pull in panel/router state.
vi.mock('../../../store/actions/repositories', () => ({
  viewChangeDiff: vi.fn(),
  viewThreadCcDiff: vi.fn(),
}));

import { viewChangeDiff, viewThreadCcDiff } from '../../../store/actions/repositories';

/** Build a close-set TaggedAction the way resolveThreadActions would. */
function ta(kind: TaggedAction['kind'], label: string, category: TaggedAction['category'] = 'close'): TaggedAction {
  return { kind, category, label, invoke: () => {} };
}
const DISCARD_APPLY = [ta('discard', 'Discard'), ta('apply', 'Apply', 'primary')];
const ARCHIVE_ONLY = [ta('archive', 'Archive')];

function makeCCThread(id: string, overrides: Partial<ThreadState['meta']> = {}): ThreadState {
  return {
    meta: {
      id,
      title: 'test',
      channel: 'claude_code',
      initiator: 'user',
      saved: false,
      createdAt: '',
      updatedAt: '',
      status: 'waiting',
      messageCount: 0,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
      ...overrides,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
  archivingThreadIds.value = new Set();
  applyingNowThreadIds.value = new Map();
  discardingCCThreadIds.value = new Set();
  cancelingThreadIds.value = new Set();
  changes.value = { status: 'loaded', data: [] };
  vi.mocked(viewChangeDiff).mockReset();
  vi.mocked(viewThreadCcDiff).mockReset();
});

function vnodeText(n: ComponentChildren): string {
  if (n === null || n === undefined || typeof n === 'boolean') return '';
  if (typeof n === 'string' || typeof n === 'number') return String(n);
  if (Array.isArray(n)) return n.map(vnodeText).join('');
  return vnodeText((n as VNode<{ children?: ComponentChildren }>).props.children);
}

function buttonLabels(node: ComponentChildren): string[] {
  if (node === null || node === undefined || typeof node === 'boolean') return [];
  if (typeof node === 'string' || typeof node === 'number') return [];
  if (Array.isArray(node)) return node.flatMap(buttonLabels);
  const v = node as VNode<{ children?: ComponentChildren }>;
  if (v.type === 'button') return [vnodeText(v.props.children).trim()];
  return buttonLabels(v.props.children);
}

function buttonNodes(node: ComponentChildren): VNode<{ disabled?: boolean }>[] {
  if (node === null || node === undefined || typeof node === 'boolean') return [];
  if (typeof node === 'string' || typeof node === 'number') return [];
  if (Array.isArray(node)) return node.flatMap(buttonNodes);
  const v = node as VNode<{ children?: ComponentChildren; disabled?: boolean }>;
  if (v.type === 'button') return [v];
  return buttonNodes(v.props.children);
}

describe('getBannerSlots', () => {
  it('actions state with Diff puts Diff alone in liftable; actions in primary', () => {
    const slots = getBannerSlots({
      type: 'actions',
      actions: DISCARD_APPLY,
      threadId: 'tid',
      isArchiving: false,
      showDiff: true,
    });

    expect(buttonLabels(slots.liftable)).toEqual(['Diff']);
    expect(buttonLabels(slots.primary)).toEqual(['Discard', 'Apply']);
  });

  it('actions state on a non-CC thread hides the Diff button', () => {
    const slots = getBannerSlots({
      type: 'actions',
      actions: ARCHIVE_ONLY,
      threadId: 'tid',
      isArchiving: false,
      showDiff: false,
    });

    expect(slots.liftable).toBeNull();
    expect(buttonLabels(slots.primary)).toEqual(['Archive']);
  });

  it('CC thread with no diff hides the Diff button entirely', () => {
    const slots = getBannerSlots({
      type: 'actions',
      actions: ARCHIVE_ONLY,
      threadId: 'tid',
      isArchiving: false,
      showDiff: false,
    });

    expect(slots.liftable).toBeNull();
    expect(buttonLabels(slots.primary)).toEqual(['Archive']);
  });

  it('CC thread with showDiff uses the thread-level diff click', () => {
    const slots = getBannerSlots({
      type: 'actions',
      actions: DISCARD_APPLY,
      threadId: 'tid',
      isArchiving: false,
      showDiff: true,
    });

    expect(buttonLabels(slots.liftable)).toEqual(['Diff']);
    const [diffBtn] = buttonNodes(slots.liftable);
    expect(diffBtn.props.disabled).toBeFalsy();
    expect(typeof (diffBtn.props as { onClick?: unknown }).onClick).toBe('function');
  });

  it('archiving state puts a disabled Archive... in primary; nothing liftable', () => {
    const slots = getBannerSlots({
      type: 'actions',
      actions: [],
      threadId: 'tid',
      isArchiving: true,
      showDiff: false,
    });
    expect(slots.liftable).toBeNull();
    expect(buttonLabels(slots.primary)).toEqual(['Archive...']);
    const [btn] = buttonNodes(slots.primary);
    expect(btn.props.disabled).toBe(true);
  });

  it('applying state puts Apply... in primary; nothing liftable', () => {
    const slots = getBannerSlots({ type: 'applying' });
    expect(slots.liftable).toBeNull();
    expect(buttonLabels(slots.primary)).toEqual(['Apply...']);
  });

  it('discarding state puts Discard... in primary; nothing liftable', () => {
    const slots = getBannerSlots({ type: 'discarding' });
    expect(slots.liftable).toBeNull();
    expect(buttonLabels(slots.primary)).toEqual(['Discard...']);
  });
});

describe('showDiff is driven by codingAgentHasDiff alone', () => {
  it('Diff is shown when codingAgentHasDiff is true and no pending change exists', () => {
    // Single-signal rule: branch-has-diff is the sole driver. The previous
    // three-signal union (pendingChange OR codingAgentProposed OR codingAgentIsExternalRepo)
    // is gone — codingAgentHasDiff is the git-truth replacement.
    const thread = makeCCThread('t1', {
      status: 'idle',
      section: 'inbox',
      codingAgentProposed: false,
      codingAgentIsExternalRepo: false,
      codingAgentHasDiff: true,
    });
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.showDiff).toBe(true);
    }
  });

  it('Diff is hidden when codingAgentHasDiff is false even if coding_agent_proposed=true', () => {
    // Proves the new rule replaces the union: under the old union,
    // codingAgentProposed=true alone would have shown Diff. The new rule
    // requires the branch to actually have a diff against its diff base.
    const thread = makeCCThread('t1', {
      status: 'waiting',
      section: 'inbox',
      codingAgentProposed: true,
      codingAgentIsExternalRepo: false,
      codingAgentHasDiff: false,
    });
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.showDiff).toBe(false);
    }
  });

  it('Diff click always routes to viewThreadCcDiff', () => {
    // Pin down that the WaitingBanner Diff button has a different conceptual
    // identity from the historical Change-row Diff buttons: it always asks
    // "show me the diff for this thread's branch", never "show me what this
    // specific Change contained". viewChangeDiff stays for ChatExchange and
    // ChangesView; the WaitingBanner does not call it anymore.
    const slots = getBannerSlots({
      type: 'actions',
      actions: DISCARD_APPLY,
      threadId: 'tid',
      isArchiving: false,
      showDiff: true,
    });

    const [diffBtn] = buttonNodes(slots.liftable);
    const onClick = (diffBtn.props as { onClick?: () => void }).onClick;
    expect(typeof onClick).toBe('function');
    onClick!();

    expect(viewThreadCcDiff).toHaveBeenCalledTimes(1);
    expect(viewThreadCcDiff).toHaveBeenCalledWith('tid');
    expect(viewChangeDiff).not.toHaveBeenCalled();
  });
});

describe('getStandaloneCcDiffButton', () => {
  // The standalone Diff button is rendered by PromptInput when the in-banner
  // Diff (from getBannerSlots) is not in play — most importantly during
  // mid-turn (waitingState='canceling'), where the banner is suppressed but
  // the branch already has commits to diff. "Branch has commits → Diff
  // visible" is the user-facing rule; the data layer already exposes that
  // truth via meta.codingAgentHasDiff, this helper just surfaces it.

  it('renders Diff button for focused CC thread with codingAgentHasDiff=true', () => {
    const thread = makeCCThread('t1', {
      status: 'running',
      codingAgentHasDiff: true,
    });
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const node = getStandaloneCcDiffButton();
    expect(buttonLabels(node)).toEqual(['Diff']);
    const [btn] = buttonNodes(node);
    expect(btn.props.disabled).toBeFalsy();
  });

  it('returns null when codingAgentHasDiff=false', () => {
    const thread = makeCCThread('t1', {
      status: 'running',
      codingAgentHasDiff: false,
    });
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    expect(getStandaloneCcDiffButton()).toBeNull();
  });

  it('returns null for non-CC (chat) thread even when something thinks it has a diff', () => {
    // chat threads cannot have CC diffs; guard belt-and-braces.
    const thread = makeCCThread('t1', {
      channel: 'chat',
      status: 'idle',
      codingAgentHasDiff: true,
    });
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    expect(getStandaloneCcDiffButton()).toBeNull();
  });

  it('returns null when no thread is focused', () => {
    focusedThreadId.value = null;
    expect(getStandaloneCcDiffButton()).toBeNull();
  });

  it('Diff click routes to viewThreadCcDiff for the focused thread', () => {
    const thread = makeCCThread('tid', {
      status: 'running',
      codingAgentHasDiff: true,
    });
    threadMap.value = new Map([['tid', thread]]);
    focusedThreadId.value = 'tid';

    const node = getStandaloneCcDiffButton();
    const [btn] = buttonNodes(node);
    const onClick = (btn.props as { onClick?: () => void }).onClick;
    expect(typeof onClick).toBe('function');
    onClick!();
    expect(viewThreadCcDiff).toHaveBeenCalledTimes(1);
    expect(viewThreadCcDiff).toHaveBeenCalledWith('tid');
  });
});
