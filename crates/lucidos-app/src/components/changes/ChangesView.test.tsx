import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
}));

import { openChangeThread, applyBlockedReason, changeRowActions, bulkApplyState, THREAD_UNSETTLED_TIP } from './ChangesView';
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
  it('offers the standing apply, and only that, while the thread is working', () => {
    const actions = changeRowActions(
      makeChange({ thread_unsettled: true, thread_working: true }),
      false,
    );
    expect(actions.map((a) => a.kind)).toEqual(['standing']);
    expect(actions[0]).toMatchObject({ label: 'Apply as it settles' });
  });

  it('flips the standing face to a cancel once armed', () => {
    const [action] = changeRowActions(
      makeChange({ thread_unsettled: true, thread_working: true }),
      true,
    );
    expect(action).toMatchObject({ kind: 'standing', label: '✓ Applying as it settles' });
  });

  // A parked thread never settles by itself, so an arm on it drops the moment
  // it is pressed. Offering one is the same broken control in a new coat.
  it('offers nothing on a change whose thread is parked rather than working', () => {
    const parked = makeChange({ thread_unsettled: true, thread_working: false });
    expect(changeRowActions(parked, false)).toEqual([]);
  });

  it('drops Apply for an emptied change and keeps Discard, which resolves it', () => {
    const actions = changeRowActions(makeChange({ file_count: 0 }), false);
    expect(actions.map((a) => a.kind)).toEqual(['discard']);
  });

  it('offers both on an ordinary settled change', () => {
    const actions = changeRowActions(makeChange(), false);
    expect(actions.map((a) => a.kind)).toEqual(['discard', 'apply']);
    expect(actions[1]).toMatchObject({ label: 'Apply' });
  });

  it('marks a restart-requiring Apply, as the old row did', () => {
    const actions = changeRowActions(makeChange({ requires_restart: true }), false);
    expect(actions[1]).toMatchObject({ kind: 'apply', label: 'Apply*' });
  });
});

describe('bulkApplyState: Apply All, and the sweep beside it', () => {
  const settled = makeChange({ id: 'a' });
  const working = makeChange({ id: 'b', thread_unsettled: true });

  it('offers nothing for a lone settled change with nothing working', () => {
    expect(bulkApplyState([settled], 0, 0).show).toBe(false);
  });

  it('offers Apply All plus the checkbox when both are true', () => {
    const state = bulkApplyState([settled, working], 1, 0);
    expect(state).toMatchObject({
      show: true,
      canApplyNow: true,
      sweepOnly: false,
      armed: false,
      offerKeepGoing: true,
      showDiscardAll: true,
    });
  });

  it('reads as the sweep alone when nothing can be applied now', () => {
    const state = bulkApplyState([working], 2, 0);
    expect(state).toMatchObject({ show: true, canApplyNow: false, sweepOnly: true });
    // No checkbox: with nothing appliable, arming IS the button.
    expect(state.offerKeepGoing).toBe(false);
  });

  it('offers the sweep with no pending changes at all', () => {
    expect(bulkApplyState([], 3, 0)).toMatchObject({ show: true, sweepOnly: true });
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
    expect(state).toMatchObject({ show: true, sweepOnly: true, armed: true });
  });

  it('withdraws the checkbox once armed, because the toggle says it', () => {
    const state = bulkApplyState([settled, working], 1, 1);
    expect(state).toMatchObject({ armed: true, canApplyNow: true, offerKeepGoing: false });
  });

  it('keeps the off reachable after the last thread stops working', () => {
    const state = bulkApplyState([], 0, 1);
    expect(state).toMatchObject({ show: true, sweepOnly: true, armed: true });
  });
});
