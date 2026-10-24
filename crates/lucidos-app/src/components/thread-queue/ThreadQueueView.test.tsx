import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
}));

import { openQueueEntryThread } from './ThreadQueueView';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import type { ThreadQueueEntry } from '../../store/types';

function makeEntry(over: Partial<ThreadQueueEntry> = {}): ThreadQueueEntry {
  return {
    id: 'entry-1',
    kind: 'user-chat',
    thread_id: 'thread-uuid-1',
    summary: 'theres stll a bug where bell/con...',
    status: 'admitted',
    queued_at: '2026-01-01T00:00:00Z',
    admitted_at: '2026-01-01T00:00:00Z',
    ...over,
  };
}

describe('openQueueEntryThread', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('routes through focusThreadOrBootstrap for the entry\'s bound thread', () => {
    openQueueEntryThread(makeEntry({ thread_id: 'thread-uuid-1' }));
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('thread-uuid-1');
  });

  it('is a no-op when the entry has no materialized thread', () => {
    openQueueEntryThread(makeEntry({ thread_id: undefined }));
    expect(focusThreadOrBootstrap).not.toHaveBeenCalled();
  });
});
