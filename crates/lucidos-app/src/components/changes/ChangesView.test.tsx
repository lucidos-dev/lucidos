import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
}));

import { openChangeThread, applyBlockedReason, THREAD_UNSETTLED_TIP } from './ChangesView';
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
