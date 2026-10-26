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

// Stub repositories actions so we can assert which one the Diff button click
// calls. Importing the real module would pull in panel/router state.
vi.mock('../../../store/actions/repositories', () => ({
  viewChangeDiff: vi.fn(),
  viewThreadCcDiff: vi.fn(),
}));

import { viewChangeDiff, viewThreadCcDiff } from '../../../store/actions/repositories';
import { _resetRowIntentsForTesting } from '../../../store/rowIntents';

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

// Diff is a COMPONENT rather than a bare <button>, because it holds a
// touch-activation hook. A vnode walker cannot look inside one without a
// renderer, so it is matched by identity and answers for its own label. What it
// DOES on a press is covered by `diff-button-touch.test.tsx`, which renders it.
// These slots are about composition: which face lands where.
function buttonLabels(node: ComponentChildren): string[] {
  if (node === null || node === undefined || typeof node === 'boolean') return [];
  if (typeof node === 'string' || typeof node === 'number') return [];
  if (Array.isArray(node)) return node.flatMap(buttonLabels);
  const v = node as VNode<{ children?: ComponentChildren }>;
  if (v.type === DiffButton) return ['Diff'];
  if (v.type === 'button') return [vnodeText(v.props.children).trim()];
  return buttonLabels(v.props.children);
}

/** The Diff faces in a slot, matched by component identity. */
function diffNodes(node: ComponentChildren): VNode<{ threadId: string }>[] {
  if (node === null || node === undefined || typeof node === 'boolean') return [];
  if (typeof node === 'string' || typeof node === 'number') return [];
  if (Array.isArray(node)) return node.flatMap(diffNodes);
  const v = node as VNode<{ children?: ComponentChildren }>;
  if (v.type === DiffButton) return [v as unknown as VNode<{ threadId: string }>];
  return diffNodes(v.props.children);
}

/** What the composer row stamps on each member it renders. */
const ROW_ATTRS = { 'data-row-item': 'fold' };

/** A member's ROW rendering, which is what the assertions below look at. */
function rowOf(member: HeaderActionSpec): ComponentChildren {
  return member.render ? member.render(ROW_ATTRS) : null;
}

/** Every member's row rendering, in fold order. */
function rows(members: HeaderActionSpec[]): ComponentChildren[] {
  return members.map(rowOf);
}

type Banner = Parameters<typeof getBannerActions>[0];

/** The action slot's actions, face first, then its menu. */
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
    // Diff is never folded into the Apply menu, which is a different thing.
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

  /** One action slot: several close actions without Apply merge too, with
   *  Archive on the face. */
  it('merges several close actions without Apply under an Archive face', () => {
    const state = actionsState([ta('discard', 'Discard'), ta('set_aside', 'Set aside'), ta('archive', 'Archive')]);
    expect(slotKeys(state)).toEqual(['archive', 'set_aside', 'discard']);
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

  /** A request in flight is the plain verb beside a spinner, disabled. */
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

  // The engine can empty a pending change (the branch no longer differs from
  // main) and the thread still reads proposed. Its file list hides Diff.
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
    // Apply All marks every batch member's change as applying (change-level
    // applyingChangeIds → reverse-mapped by applyingChangeThreadIds). A member
    // still waiting its turn has no live session (status 'waiting', not mid-turn)
    // — nothing to interrupt — so it shows the disabled "Apply...".
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
    // The in-flight Apply All member runs /harden as a live CC turn (status
    // 'running' = mid-turn). The user must be able to cancel it — the mid-turn
    // branch wins over the applyingChangeThreadIds disabled state.
    const thread = makeCCThread('t1', { status: 'running' });
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    changes.value = { status: 'loaded', data: [{ id: 'c1', thread_id: 't1' } as never] };
    applyingChangeIds.value = new Set(['c1']);

    const state = getWaitingState();
    expect(state!.type).toBe('canceling');
  });

  it('shows Cancel (not disabled applying) while a merge-conflict resolution runs', () => {
    // Status 'running' during an apply-driven merge. Cancel
    // is best-effort but must be offered (regression fix: it used to be disabled).
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
    // Pin down that the WaitingBanner Diff button has a different conceptual
    // identity from the historical Change-row Diff buttons: it always asks
    // "show me the diff for this thread's branch", never "show me what this
    // specific Change contained". viewChangeDiff stays for ChatExchange and
    // ChangesView; the WaitingBanner does not call it anymore. Diff is the
    // own member on every actions path (Apply or not), so this asserts the one
    // `diffAction` face both paths share.
    const members = getBannerActions(actionsState(ARCHIVE_ONLY, { showDiff: true })).situational;

    // One face, carrying the THREAD id. What it calls with that id is asserted
    // against the rendered component in `diff-button-touch.test.tsx`.
    expect(diffNodes(rows(members)).map((v) => v.props.threadId)).toEqual(['tid']);
    expect(viewChangeDiff).not.toHaveBeenCalled();
  });
});

describe('getStandaloneActions', () => {
  // The standalone members are what PromptInput carries when the in-banner set
  // (from getBannerActions) is not in play, most importantly during
  // mid-turn (waitingState='canceling'), where the banner is suppressed but
  // the branch already has commits to diff. "Branch has commits → Diff
  // visible" is the user-facing rule; the data layer already exposes that
  // truth via meta.codingAgentChangeState, this helper just surfaces it.

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
    // chat threads cannot have CC diffs; guard belt-and-braces.
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
    // The standalone path resolves the thread itself, where the banner path is
    // handed one. Both must reach the same face with the same id.
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

// A set-aside change keeps a way back on its own thread, even once the thread
// is archived and offers nothing else (ADR 0328).
describe('a thread holding a set-aside change', () => {
  it('offers Bring back, archived or not', () => {
    const thread = makeCCThread('t-aside', { status: 'idle', section: 'archived' });
    threadMap.value = new Map([['t-aside', thread]]);
    focusedThreadId.value = 't-aside';
    setAsideChanges.value = {
      status: 'loaded',
      data: [{ id: 'c-aside', thread_id: 't-aside', status: 'set_aside' } as never],
    };
    const state = getWaitingState();
    expect(state).toMatchObject({ type: 'actions', setAside: { changeId: 'c-aside', discardable: true } });
    expect(slotKeys(state as Banner)).toEqual(['bring-back', 'discard-set-aside']);
  });

  const withSetAside = (actions: TaggedAction[], discardable: boolean) =>
    actionsState(actions, { threadId: 't-aside', setAside: { changeId: 'c-aside', discardable } });

  // The same pair the Changes panel's set-aside row offers.
  it('draws Bring back on the face with Discard in its menu', () => {
    const { face, menu, menuLabel } = getBannerActions(withSetAside([], true)).slot;
    expect(face.label).toBe('Bring back');
    expect(menu.map((a) => [a.label, a.tone])).toEqual([['Discard', 'danger']]);
    expect(menuLabel).toBe('More set-aside actions');
  });

  it('withholds Discard while the thread is unsettled', () => {
    expect(slotKeys(withSetAside([], false))).toEqual(['bring-back']);
  });

  it('keeps Archive on the face and Bring back in its menu', () => {
    expect(slotKeys(withSetAside(ARCHIVE_ONLY, true))).toEqual(['archive', 'bring-back', 'discard-set-aside']);
  });

  // A thread watching an event is idle, so the banner shows, but the engine
  // refuses the discard until the thread settles.
  it('reads a thread watching an event as unsettled', () => {
    const thread = makeCCThread('t-watch', { status: 'idle', liveEventWaitCount: 1 });
    threadMap.value = new Map([['t-watch', thread]]);
    focusedThreadId.value = 't-watch';
    setAsideChanges.value = {
      status: 'loaded',
      data: [{ id: 'c-watch', thread_id: 't-watch', status: 'set_aside' } as never],
    };
    expect(getWaitingState()).toMatchObject({ setAside: { changeId: 'c-watch', discardable: false } });
  });

  it('offers nothing to bring back when the thread has no set-aside change', () => {
    const thread = makeCCThread('t-none', { status: 'idle', section: 'archived' });
    threadMap.value = new Map([['t-none', thread]]);
    focusedThreadId.value = 't-none';
    expect(getWaitingState()).toBeNull();
  });
});

// A blocked or finished Archive never leaves the composer row silent (ADR 0378).
describe('the composer row when Archive is not on offer', () => {
  // The thread's ⋯ menu moves it back to Current, so the composer keeps Send.
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
    // Shown in Current, so it is not archived as the reader sees it. Archive
    // makes the engine recount the family, which lets it go to Archive.
    threadMap.value = new Map([
      ['t-coord', makeCCThread('t-coord', { status: 'idle', section: 'archived', activeChildrenCount: 12 })],
    ]);
    focusedThreadId.value = 't-coord';
    const state = getWaitingState();
    expect(state).toMatchObject({ type: 'actions' });
    expect(slotKeys(state as Banner)).toEqual(['archive']);
  });

  it('draws nothing extra when the thread itself holds Archive back', () => {
    // Its own blocker already draws the control that resolves it.
    threadMap.value = new Map([['t-run', makeCCThread('t-run', { status: 'running' })]]);
    focusedThreadId.value = 't-run';
    expect(getWaitingState()).toMatchObject({ type: 'canceling' });
  });
});
