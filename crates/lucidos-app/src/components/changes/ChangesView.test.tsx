import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
}));

import { openChangeThread } from './ChangesView';
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
