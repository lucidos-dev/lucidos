import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { getBannerActions, getWaitingState, getStandaloneActions, DiffButton } from '../WaitingBanner';
import type { HeaderActionSpec } from '../../layout/headerActions';
import {
  threadMap,
  focusedThreadId,
  applyNowRequests,
  applyingChangeIds,
  discardingCCThreadIds,
  cancelingThreadIds,
  changes,
  setAsideChanges,
} from '../../../store/store';
import type { ThreadState } from '../../../store/thread-events';
import type { TaggedAction } from '../../../store/actions/threadActions';

vi.mock('../../../store/actions/repositories', () => ({
  viewChangeDiff: vi.fn(),
  viewThreadCcDiff: vi.fn(),
}));

import { viewChangeDiff, viewThreadCcDiff } from '../../../store/actions/repositories';
import { _resetRowIntentsForTesting } from '../../../store/rowIntents';

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
      summaryVersion: 0,
      messageCount: 0,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
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
  _resetRowIntentsForTesting();
  applyNowRequests.value = new Map();
  applyingChangeIds.value = new Set();
  discardingCCThreadIds.value = new Set();
  cancelingThreadIds.value = new Set();
  changes.value = { status: 'loaded', data: [] };
  setAsideChanges.value = { status: 'loaded', data: [] };
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
  if (v.type === DiffButton) return ['Diff'];
  if (v.type === 'button') return [vnodeText(v.props.children).trim()];
  return buttonLabels(v.props.children);
}

function diffNodes(node: ComponentChildren): VNode<{ threadId: string }>[] {
  if (node === null || node === undefined || typeof node === 'boolean') return [];
  if (typeof node === 'string' || typeof node === 'number') return [];
  if (Array.isArray(node)) return node.flatMap(diffNodes);
  const v = node as VNode<{ children?: ComponentChildren }>;
  if (v.type === DiffButton) return [v as unknown as VNode<{ threadId: string }>];
  return diffNodes(v.props.children);
}

const ROW_ATTRS = { 'data-row-item': 'fold' };

function rowOf(member: HeaderActionSpec): ComponentChildren {
  return member.render ? member.render(ROW_ATTRS) : null;
}

function rows(members: HeaderActionSpec[]): ComponentChildren[] {
  return members.map(rowOf);
}

type Banner = Parameters<typeof getBannerActions>[0];

function slotKeys(state: Banner): string[] {
  const { face, menu } = getBannerActions(state).slot;
  return [face.key, ...menu.map((a) => a.key)];
}

const actionsState = (actions: TaggedAction[], extra: Partial<Extract<Banner, { type: 'actions' }>> = {}): Banner => ({
  type: 'actions', actions, threadId: 'tid', isArchiving: false, showDiff: false, ...extra,
});

describe('getBannerActions', () => {
  it('keeps Diff a foldable member, out of the action slot', () => {
    const { situational, slot } = getBannerActions(actionsState(DISCARD_APPLY, { showDiff: true }));
    expect(situational.map((m) => m.key)).toEqual(['thread-diff']);
    expect(buttonLabels(rowOf(situational[0]))).toEqual(['Diff']);
    expect(diffNodes(rows(situational)).map((v) => v.props.threadId)).toEqual(['tid']);
    expect(slot.face.key).toBe('apply');
  });

  it('drops the Diff member when the branch has no diff', () => {
    expect(getBannerActions(actionsState(DISCARD_APPLY)).situational).toEqual([]);
  });

  it('puts Apply on the face and the other close actions in its menu, Discard last', () => {
    const state = actionsState([ta('discard', 'Discard'), ta('archive', 'Archive'), ta('apply', 'Apply*', 'primary')]);
    expect(slotKeys(state)).toEqual(['apply', 'archive', 'discard']);
    const { face, menuLabel } = getBannerActions(state).slot;
    expect(face.label).toBe('Apply*');
    expect(face.tone).toBe('confirm');
    expect(menuLabel).toBe('More change actions');
  });

  it('merges several close actions without Apply under an Archive face', () => {
    const state = actionsState([ta('discard', 'Discard'), ta('hold', 'Hold…'), ta('archive', 'Archive')]);
    expect(slotKeys(state)).toEqual(['archive', 'hold', 'discard']);
    expect(getBannerActions(state).slot.face.ariaLabel).toBe('Archive thread');
  });

  it('draws a lone close action with no menu', () => {
    expect(slotKeys(actionsState(ARCHIVE_ONLY))).toEqual(['archive']);
  });

  it('runs the selector action on a press', () => {
    const invoke = vi.fn();
    const { face } = getBannerActions(actionsState([{ ...ta('archive', 'Archive'), invoke }])).slot;
    face.invoke!();
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it.each([
    [{ type: 'applying' } as Banner, 'Apply', 'Applying'],
    [{ type: 'discarding' } as Banner, 'Discard', 'Discarding'],
    [actionsState([], { isArchiving: true }), 'Archive', 'Archiving'],
  ])('gives a busy thread its plain verb, disabled', (state, label, name) => {
    const { situational, slot } = getBannerActions(state);
    expect(situational).toEqual([]);
    expect(slot.menu).toEqual([]);
    expect(slot.face.label).toBe(label);
    expect(slot.face.ariaLabel).toBe(name);
    expect(slot.face.invoke).toBeNull();
  });
});

describe('showDiff follows the change state', () => {
  it('Diff is shown for unproposed work with no pending change', () => {
    const thread = makeCCThread('t1', {
      status: 'idle',
      section: 'inbox',
      codingAgentChangeState: { kind: 'unproposed', reason: null },
      codingAgentIsExternalRepo: false,
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

  it('Diff is hidden for a proposal whose pending change lists no files', () => {
    const thread = makeCCThread('t1', {
      status: 'waiting',
      section: 'inbox',
      codingAgentChangeState: { kind: 'proposed', requires_restart: false },
      codingAgentIsExternalRepo: false,
    });
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    changes.value = {
      status: 'loaded',
      data: [{ id: 'c1', thread_id: 't1', status: 'pending', file_count: 0, files: [] } as never],
    };

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.showDiff).toBe(false);
    }
  });

  it('Diff is shown for a proposal whose pending change lists files', () => {
    const thread = makeCCThread('t1', {
      status: 'waiting',
      section: 'inbox',
      codingAgentChangeState: { kind: 'proposed', requires_restart: false },
    });
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    changes.value = {
      status: 'loaded',
      data: [{ id: 'c1', thread_id: 't1', status: 'pending', file_count: 1, files: ['a.rs'] } as never],
    };

    const state = getWaitingState();
    expect(state?.type === 'actions' && state.showDiff).toBe(true);
  });

  it('Diff is hidden when the branch holds no work', () => {
    const thread = makeCCThread('t1', { status: 'idle', section: 'inbox' });
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state?.type).toBe('actions');
    expect(state?.type === 'actions' && state.showDiff).toBe(false);
  });

  it('shows disabled "applying" for a QUEUED Apply All member (change applying, thread idle/waiting)', () => {
    const thread = makeCCThread('t1', { status: 'waiting', codingAgentChangeState: { kind: 'unproposed', reason: null }});
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    changes.value = { status: 'loaded', data: [{ id: 'c1', thread_id: 't1' } as never] };
    applyingChangeIds.value = new Set(['c1']);

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('applying');
  });

  it('shows Cancel (not disabled applying) when this thread\'s change is actively hardening', () => {
    const thread = makeCCThread('t1', { status: 'running' });
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    changes.value = { status: 'loaded', data: [{ id: 'c1', thread_id: 't1' } as never] };
    applyingChangeIds.value = new Set(['c1']);

    const state = getWaitingState();
    expect(state!.type).toBe('canceling');
  });

  it('shows Cancel (not disabled applying) while a merge-conflict resolution runs', () => {
    const thread = makeCCThread('t1', { status: 'running' });
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state!.type).toBe('canceling');
  });

  it('does not show "applying" when the applying change belongs to a different thread', () => {
    const thread = makeCCThread('t1', { status: 'waiting' });
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    changes.value = { status: 'loaded', data: [{ id: 'c2', thread_id: 't2' } as never] };
    applyingChangeIds.value = new Set(['c2']);

    const state = getWaitingState();
    expect(state!.type).not.toBe('applying');
  });

  it('Diff click always routes to viewThreadCcDiff', () => {
    const members = getBannerActions(actionsState(ARCHIVE_ONLY, { showDiff: true })).situational;

    expect(diffNodes(rows(members)).map((v) => v.props.threadId)).toEqual(['tid']);
    expect(viewChangeDiff).not.toHaveBeenCalled();
  });
});

describe('getStandaloneActions', () => {

  it('renders Diff button for a focused CC thread holding unproposed work', () => {
    const thread = makeCCThread('t1', {
      status: 'running',
      codingAgentChangeState: { kind: 'unproposed', reason: null },
    });
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const members = getStandaloneActions();
    expect(members.map((m) => m.key)).toContain('thread-diff');
    expect(diffNodes(rows(members)).map((v) => v.props.threadId)).toEqual(['t1']);
  });

  it('returns null when the branch holds no work', () => {
    const thread = makeCCThread('t1', {
      status: 'running',
      codingAgentChangeState: { kind: 'none' },
    });
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    expect(getStandaloneActions().map((m) => m.key)).not.toContain('thread-diff');
  });

  it('returns null for non-CC (chat) thread even when something thinks it has a diff', () => {
    const thread = makeCCThread('t1', {
      channel: 'chat',
      status: 'idle',
      codingAgentChangeState: { kind: 'unproposed', reason: null },
    });
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    expect(getStandaloneActions().map((m) => m.key)).not.toContain('thread-diff');
  });

  it('returns null when no thread is focused', () => {
    focusedThreadId.value = null;
    expect(getStandaloneActions()).toEqual([]);
  });

  it('hands the FOCUSED thread id to the Diff face', () => {
    const thread = makeCCThread('tid', {
      status: 'running',
      codingAgentChangeState: { kind: 'unproposed', reason: null },
    });
    threadMap.value = new Map([['tid', thread]]);
    focusedThreadId.value = 'tid';

    expect(diffNodes(rows(getStandaloneActions())).map((v) => v.props.threadId)).toEqual(['tid']);
    expect(viewChangeDiff).not.toHaveBeenCalled();
  });
});

describe('a thread holding a set-aside change', () => {
  it('draws no banner for it, archived or not', () => {
    const thread = makeCCThread('t-aside', { status: 'idle', section: 'archived' });
    threadMap.value = new Map([['t-aside', thread]]);
    focusedThreadId.value = 't-aside';
    setAsideChanges.value = {
      status: 'loaded',
      data: [{ id: 'c-aside', thread_id: 't-aside', status: 'set_aside' } as never],
    };
    expect(getWaitingState()).toBeNull();
  });
});

describe('the composer row when Archive is not on offer', () => {
  it('draws no banner on an archived thread', () => {
    threadMap.value = new Map([['t-arch', makeCCThread('t-arch', { status: 'idle', section: 'archived' })]]);
    focusedThreadId.value = 't-arch';
    expect(getWaitingState()).toBeNull();
  });

  it('shows Archive held back, with its reason, when a sub-thread blocks it', () => {
    threadMap.value = new Map([
      ['t-parent', makeCCThread('t-parent', { status: 'idle', blockingDescendantCount: 1 })],
      ['t-child', makeCCThread('t-child', { status: 'waiting_for_user_answer', parentThreadId: 't-parent' })],
    ]);
    focusedThreadId.value = 't-parent';
    const state = getWaitingState();
    expect(state).toMatchObject({
      type: 'actions',
      blockedArchive: { blocker: 'descendant_question', subThreadId: 't-child' },
    });
    const { face, menu } = getBannerActions(state as Banner).slot;
    expect(menu).toEqual([]);
    expect(face.key).toBe('archive-blocked');
    expect(face.heldBack).toBe(true);
    expect(face.tooltip).toBe('A sub-thread is waiting for your answer.');
  });

  it('offers Archive on a stored-archived thread a stale count keeps in Current', () => {
    threadMap.value = new Map([
      ['t-coord', makeCCThread('t-coord', { status: 'idle', section: 'archived', activeChildrenCount: 12 })],
    ]);
    focusedThreadId.value = 't-coord';
    const state = getWaitingState();
    expect(state).toMatchObject({ type: 'actions' });
    expect(slotKeys(state as Banner)).toEqual(['archive']);
  });

  it('draws nothing extra when the thread itself holds Archive back', () => {
    threadMap.value = new Map([['t-run', makeCCThread('t-run', { status: 'running' })]]);
    focusedThreadId.value = 't-run';
    expect(getWaitingState()).toMatchObject({ type: 'canceling' });
  });
});
