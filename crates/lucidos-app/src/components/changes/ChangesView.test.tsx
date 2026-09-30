import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
}));

import { openChangeThread, applyBlockedReason, changeRowActions, rowActionClass, rowActionLayout, bulkApplyState, THREAD_UNSETTLED_TIP } from './ChangesView';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import type { Change } from '../../api/client';

function makeChange(over: Partial<Change> = {}): Change {
  return {
    id: 'change-1',
    request_id: '00000000-0000-0000-0000-000000000000',
    thread_id: 'thread-uuid-1',
    thread_title: null,
    branch_name: 'b',
    repo_root: '/r',
    description: 'desc',
    file_count: 1,
    files: ['a.rs'],
    requires_restart: false,
    hardened: true,
    status: 'pending',
    created_at: '2026-01-01T00:00:00Z',
    resolved_at: null,
    pre_merge_sha: null,
    post_merge_sha: null,
    commits: [],
    summary: null,
    incomplete: false,
    ...over,
  };
}

describe('openChangeThread', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('routes through focusThreadOrBootstrap, deep-linking to the change\'s own diff event', () => {
    // targetChangeId (the change row's id), NOT the bottom of the thread — the
    // change isn't necessarily the thread's last turn.
    openChangeThread(makeChange({ thread_id: 'thread-uuid-1', id: 'change-7' }));
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('thread-uuid-1', { targetChangeId: 'change-7' });
  });

  it('is a no-op when the change has no originating thread', () => {
    openChangeThread(makeChange({ thread_id: null }));
    expect(focusThreadOrBootstrap).not.toHaveBeenCalled();
  });
});

describe('applyBlockedReason — the UI mirror of the server-side Apply gates', () => {
  it('allows Apply on an ordinary pending change', () => {
    expect(applyBlockedReason(makeChange())).toBeNull();
  });

  it('blocks Apply while the coding agent is still working', () => {
    expect(applyBlockedReason(makeChange({ thread_unsettled: true }))).toBe(THREAD_UNSETTLED_TIP);
  });

  it('blocks Apply on a change reconciled to zero files, steering to Discard', () => {
    // Its branch commits cancelled out, so the Diff is empty. Merging would
    // only push no-op commits and could spend a harden run on nothing; the
    // per-change endpoint 409s it and Apply All filters it out.
    const reason = applyBlockedReason(makeChange({ file_count: 0, files: [] }));
    expect(reason).toBe('This change has no file changes left — discard it');
  });

  it('reports the live thread first when a change is both empty and mid-turn', () => {
    // Wait-for-the-agent is the actionable instruction; the file count may
    // still change before it idles.
    const reason = applyBlockedReason(makeChange({ file_count: 0, thread_unsettled: true }));
    expect(reason).toBe(THREAD_UNSETTLED_TIP);
  });
});

describe('changeRowActions: never a disabled change action', () => {
  it('offers the standing apply, and only that, while the thread is settling', () => {
    const actions = changeRowActions(
      makeChange({ thread_unsettled: true, thread_settling: true }),
      false,
    );
    expect(actions.map((a) => a.kind)).toEqual(['standing']);
    expect(actions[0]).toMatchObject({ label: 'Apply on settle' });
  });

  it('flips the standing face to a cancel once armed', () => {
    const [action] = changeRowActions(
      makeChange({ thread_unsettled: true, thread_settling: true }),
      true,
    );
    expect(action).toMatchObject({ kind: 'standing', label: '✓ Applying on settle' });
  });

  // A question card never settles by itself, so an arm on it drops the moment
  // it is pressed. Offering one is the same broken control in a new coat.
  it('offers nothing on a change whose thread is parked on a question', () => {
    const parked = makeChange({ thread_unsettled: true, thread_settling: false });
    expect(changeRowActions(parked, false)).toEqual([]);
  });

  // The thread is working only to finish an apply that hit a conflict, and the
  // resolver lands the change itself. That apply is already in flight.
  it('offers nothing on a change whose apply is resolving merge conflicts', () => {
    const resolving = makeChange({ thread_unsettled: true, thread_settling: true, resolving_conflict: true });
    expect(changeRowActions(resolving, false)).toEqual([]);
  });

  it('drops Apply for an emptied change and keeps Discard, which resolves it', () => {
    const actions = changeRowActions(makeChange({ file_count: 0 }), false);
    expect(actions.map((a) => a.kind)).toEqual(['discard']);
  });

  it('offers Discard, Set aside and Apply on an ordinary settled change', () => {
    const actions = changeRowActions(makeChange(), false);
    expect(actions.map((a) => a.kind)).toEqual(['discard', 'set-aside', 'apply']);
    expect(actions[2]).toMatchObject({ label: 'Apply' });
  });

  it('marks a restart-requiring Apply, as the old row did', () => {
    const actions = changeRowActions(makeChange({ requires_restart: true }), false);
    expect(actions[2]).toMatchObject({ kind: 'apply', label: 'Apply*' });
  });

  it('offers no Set aside where Discard is withheld, as the engine gate does', () => {
    const working = makeChange({ thread_unsettled: true, thread_settling: true });
    expect(changeRowActions(working, false).map((a) => a.kind)).not.toContain('set-aside');
    const emptied = makeChange({ file_count: 0 });
    expect(changeRowActions(emptied, false).map((a) => a.kind)).toEqual(['discard']);
  });

  // The thread's own change menu draws Set aside as the neutral blue button.
  it('colours Set aside the way the thread does', () => {
    expect(rowActionClass({ kind: 'set-aside' }, false)).toBe('action-btn');
  });
});

// The row draws the thread's own change button: Apply, with the rest behind
// its caret and Discard furthest from the face.
describe('rowActionLayout', () => {
  it('folds Set aside and Discard behind the Apply caret', () => {
    const layout = rowActionLayout(changeRowActions(makeChange({ requires_restart: true }), false));
    expect(layout).toMatchObject({ kind: 'split', primary: { kind: 'apply', label: 'Apply*' } });
    expect(layout.kind === 'split' && layout.menu.map((a) => a.kind)).toEqual(['set-aside', 'discard']);
  });

  it('keeps plain buttons where there is no Apply', () => {
    const emptied = rowActionLayout(changeRowActions(makeChange({ file_count: 0 }), false));
    expect(emptied).toEqual({ kind: 'flat', buttons: [{ kind: 'discard' }] });
    const working = makeChange({ thread_unsettled: true, thread_settling: true });
    expect(rowActionLayout(changeRowActions(working, false)).kind).toBe('flat');
  });
});

describe('bulkApplyState: Apply All, and the sweep beside it', () => {
  const settled = makeChange({ id: 'a' });
  const working = makeChange({ id: 'b', thread_unsettled: true });

  it('offers nothing for a lone settled change with nothing working', () => {
    expect(bulkApplyState([settled], 0, 0).show).toBe(false);
  });

  it('never counts incomplete work as appliable in bulk', () => {
    const stopped = makeChange({ id: 'c', incomplete: true });
    expect(bulkApplyState([stopped], 0, 0).canApplyNow).toBe(false);
    expect(bulkApplyState([stopped, settled], 0, 0).canApplyNow).toBe(true);
  });

  it('offers Discard All, the sweep and Apply All when both are true', () => {
    const state = bulkApplyState([settled, working], 1, 0);
    expect(state).toEqual({
      show: true,
      canApplyNow: true,
      offerSweep: true,
      armed: false,
      showApplyAll: true,
      showDiscardAll: true,
    });
  });

  it('offers the sweep alone when nothing can be applied now', () => {
    const state = bulkApplyState([working], 2, 0);
    expect(state).toMatchObject({ show: true, canApplyNow: false, offerSweep: true, showApplyAll: false });
  });

  // The sweep's own request sets the in-flight flag too. Drawing Apply All's
  // "Applying..." for it would flash a faded pill beside the toggle.
  it('keeps Apply All hidden while the sweep alone is in flight', () => {
    expect(bulkApplyState([working], 1, 0, true).showApplyAll).toBe(false);
  });

  it('keeps Apply All drawn while a batch runs with nothing left to sweep', () => {
    expect(bulkApplyState([settled, working], 0, 0, true).showApplyAll).toBe(true);
    expect(bulkApplyState([settled], 1, 0).showApplyAll).toBe(true);
  });

  it('offers the sweep with no pending changes at all', () => {
    expect(bulkApplyState([], 3, 0)).toMatchObject({ show: true, offerSweep: true });
  });

  it('offers nothing with no pending changes and nothing working', () => {
    expect(bulkApplyState([], 0, 0).show).toBe(false);
  });

  it('keeps Discard All to the multi-change case it has always had', () => {
    expect(bulkApplyState([settled], 1, 0).showDiscardAll).toBe(false);
    expect(bulkApplyState([settled, working], 0, 0).showDiscardAll).toBe(true);
  });

  // The bug this replaced: the sweep control had one face, so a press could
  // only ever re-arm.
  it('flips the sweep to its cancel face once anything is armed', () => {
    const state = bulkApplyState([working], 2, 1);
    expect(state).toMatchObject({ show: true, offerSweep: true, armed: true });
  });

  it('keeps Apply All beside the armed toggle while a change is ready', () => {
    const state = bulkApplyState([settled, working], 1, 1);
    expect(state).toMatchObject({ armed: true, canApplyNow: true, offerSweep: true });
  });

  it('keeps the off reachable after the last thread stops working', () => {
    const state = bulkApplyState([], 0, 1);
    expect(state).toMatchObject({ show: true, offerSweep: true, armed: true });
  });
});
