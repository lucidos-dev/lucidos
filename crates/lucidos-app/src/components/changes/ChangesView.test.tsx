import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
}));

import { openChangeThread, applyBlockedReason, changeRowActions, rowActionClass, rowActionLayout, pendingSections, THREAD_UNSETTLED_TIP } from './ChangesView';
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
    needs_hardening: false,
    apply_ready: true,
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
    expect(applyBlockedReason(makeChange({ thread_unsettled: true, apply_ready: false }))).toBe(THREAD_UNSETTLED_TIP);
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
    const reason = applyBlockedReason(makeChange({ file_count: 0, thread_unsettled: true, apply_ready: false }));
    expect(reason).toBe(THREAD_UNSETTLED_TIP);
  });
});

describe('changeRowActions: never a disabled change action', () => {
  it('offers the standing apply, and only that, while the thread is settling', () => {
    const actions = changeRowActions(
      makeChange({ thread_unsettled: true, apply_ready: false, thread_settling: true }),
      false,
    );
    expect(actions.map((a) => a.kind)).toEqual(['standing']);
    expect(actions[0]).toMatchObject({ label: 'Apply on settle' });
  });

  it('flips the standing face to a cancel once armed', () => {
    const [action] = changeRowActions(
      makeChange({ thread_unsettled: true, apply_ready: false, thread_settling: true }),
      true,
    );
    expect(action).toMatchObject({ kind: 'standing', label: '✓ Applying on settle' });
  });

  // A question card never settles by itself, so an arm on it drops the moment
  // it is pressed. Offering one is the same broken control in a new coat.
  it('offers nothing on a change whose thread is parked on a question', () => {
    const parked = makeChange({ thread_unsettled: true, apply_ready: false, thread_settling: false });
    expect(changeRowActions(parked, false)).toEqual([]);
  });

  // The thread is working only to finish an apply that hit a conflict, and the
  // resolver lands the change itself. That apply is already in flight.
  it('offers nothing on a change whose apply is resolving merge conflicts', () => {
    const resolving = makeChange({ thread_unsettled: true, apply_ready: false, thread_settling: true, resolving_conflict: true });
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
    const working = makeChange({ thread_unsettled: true, apply_ready: false, thread_settling: true });
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
    const working = makeChange({ thread_unsettled: true, apply_ready: false, thread_settling: true });
    expect(rowActionLayout(changeRowActions(working, false)).kind).toBe('flat');
  });
});

describe('pendingSections: Ready, then Not finished', () => {
  const settled = makeChange({ id: 'a', thread_id: 't-a' });
  const settling = makeChange({ id: 'b', thread_id: 't-b', thread_unsettled: true, apply_ready: false, thread_settling: true });
  const parked = makeChange({ id: 'c', thread_id: 't-c', thread_unsettled: true, apply_ready: false, thread_settling: false });
  const resolving = makeChange({
    id: 'd', thread_id: 't-d', thread_unsettled: true, apply_ready: false, thread_settling: true, resolving_conflict: true,
  });
  const none = new Set<string>();

  it('puts every change in exactly one section, split on whether its thread finished', () => {
    const s = pendingSections([settled, settling, parked, resolving], none);
    expect(s.ready.map((c) => c.id)).toEqual(['a']);
    expect(s.notFinished.map((c) => c.id)).toEqual(['b', 'c', 'd']);
  });

  // Ready offers Apply, so it lists only what Apply merges as it stands. The
  // engine's `apply_ready` decides it, never a rule of the panel's own.
  it('keeps a finished change that still needs hardening out of Ready', () => {
    const unhardened = makeChange({ id: 'e', thread_id: 't-e', needs_hardening: true, apply_ready: false });
    const s = pendingSections([settled, unhardened], none);
    expect(s.ready.map((c) => c.id)).toEqual(['a']);
    expect(s.notFinished.map((c) => c.id)).toEqual(['e']);
  });

  // A question card never settles by itself, so the change is not ready either.
  it('never counts a parked thread as ready', () => {
    expect(pendingSections([parked], none).ready).toEqual([]);
  });

  it('keeps Discard All to Ready, and to more than one change there', () => {
    expect(pendingSections([settled, settling], none).showDiscardAll).toBe(false);
    const other = makeChange({ id: 'f', thread_id: 't-f' });
    expect(pendingSections([settled, other, settling], none).showDiscardAll).toBe(true);
  });

  // Nothing ready offers nothing to apply in bulk, until a batch runs.
  it('offers Apply All while something is ready, or a batch still runs', () => {
    expect(pendingSections([settled], none).showApplyAll).toBe(true);
    expect(pendingSections([parked], none).showApplyAll).toBe(false);
    expect(pendingSections([parked], none, true).showApplyAll).toBe(true);
  });

  // A parked arm drops at once, and a conflict being resolved already applies.
  it('arms only the settling changes, never a parked or resolving one', () => {
    expect(pendingSections([settling, parked, resolving], none).armable.map((c) => c.id)).toEqual(['b']);
  });

  it('wears the cancel face only once every armable change is armed', () => {
    const other = makeChange({ id: 'g', thread_id: 't-g', thread_unsettled: true, apply_ready: false, thread_settling: true });
    expect(pendingSections([settling, other], new Set(['t-b'])).armed).toBe(false);
    expect(pendingSections([settling, other], new Set(['t-b', 't-g'])).armed).toBe(true);
  });

  // An arm on a thread outside the section is not this control's to show.
  it('ignores arms on threads it does not list', () => {
    expect(pendingSections([settling], new Set(['elsewhere'])).armed).toBe(false);
    expect(pendingSections([], new Set(['elsewhere'])).armed).toBe(false);
  });
});
