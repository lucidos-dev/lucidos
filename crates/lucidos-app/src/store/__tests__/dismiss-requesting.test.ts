import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  threadMap,
  focusedThreadId,
  applyingNowThreadIds,
  applyNowRequests,
  discardingCCThreadIds,
  cancelingThreadIds,
  changes,
} from '../store';
import type { ThreadState } from '../thread-events';
import { getWaitingState } from '../../components/chat/WaitingBanner';

vi.mock('../../api/threads', () => ({
  archiveThread: vi.fn().mockResolvedValue({ archived: [] }),
  saveThread: vi.fn(),
}));

vi.mock('../actions/thread-loading', () => ({
  loadThreadEvents: vi.fn(),
}));

vi.mock('../../components/chat/promptFocus', () => ({
  focusPromptNow: vi.fn(),
}));

import { handleArchiveThread } from '../actions/threads';
import { _holdArchiveForTesting, _resetRowIntentsForTesting, archiveRequestInFlight } from '../rowIntents';

function makeCCThread(id: string, status: 'idle' | 'running' | 'waiting' | 'waiting_for_user_answer', section: 'archived' | 'inbox'): ThreadState {
  return {
    meta: {
      id,
      title: 'test',
      channel: 'claude_code',
      initiator: 'user',
      saved: false,
      createdAt: '',
      updatedAt: '',
      status,
      summaryVersion: 0,
      messageCount: 0,
      section,
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
  discardingCCThreadIds.value = new Set();
  cancelingThreadIds.value = new Set();
  changes.value = { status: 'loaded', data: [] };
});

describe('Done dismiss does not flash Requesting state', () => {
  it('returns dismiss-in-progress state even after SSE changes status to idle', () => {
    const thread = makeCCThread('t1', 'idle', 'archived');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    _holdArchiveForTesting(['t1']);

    const state = getWaitingState();

    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.isArchiving).toBe(true);
      expect(state!.actions).toEqual([]);
    }
  });

  it('draws no banner when the thread sits in the Archive section', () => {
    const thread = makeCCThread('t1', 'idle', 'archived');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    expect(getWaitingState()).toBeNull();
  });

  it('returns actions when thread is waiting+inbox and NOT dismissing', () => {
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.actions.map((a) => a.kind)).toContain('archive');
      expect(state!.isArchiving).toBe(false);
    }
  });

  it('refuses to archive while an apply runs — states are mutually exclusive', async () => {
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    thread.meta.applyInFlight = true;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    await handleArchiveThread('t1');

    expect(applyingNowThreadIds.value.has('t1')).toBe(true);
    expect(archiveRequestInFlight('t1')).toBe(false);
    expect(threadMap.value.get('t1')!.meta.section).toBe('inbox');
  });
});

describe('Apply button never shows Requesting label', () => {
  it('returns applying state (always renders "Apply...")', () => {
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    applyNowRequests.value = new Map([['t1', null]]);

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('applying');
  });
});

describe('Discard hides Apply and shows "Discard..."', () => {
  it('returns discarding state when discardingCCThreadIds is set', () => {
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    thread.meta.codingAgentChangeState = { kind: 'proposed', requires_restart: false };
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    discardingCCThreadIds.value = new Set(['t1']);

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('discarding');
  });

  it('discarding takes priority over action buttons', () => {
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    discardingCCThreadIds.value = new Set(['t1']);

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('discarding');
  });

  it('applying takes priority over discarding (should not coexist)', () => {
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    applyNowRequests.value = new Map([['t1', null]]);
    discardingCCThreadIds.value = new Set(['t1']);

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('applying');
  });
});

describe('WaitingForUserAnswer surfaces Cancel so the user can abandon the question', () => {
  it('shows Cancel (only) when CC is paused on AskUserQuestion', () => {
    const thread = makeCCThread('t1', 'waiting_for_user_answer', 'inbox');
    thread.meta.codingAgentChangeState = { kind: 'proposed', requires_restart: false };
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('canceling');
    if (state!.type === 'canceling') {
      expect(state!.threadId).toBe('t1');
      expect(state!.isCanceling).toBe(false);
    }
  });

  it('reflects optimistic isCanceling=true while still in waiting_for_user_answer', () => {
    const thread = makeCCThread('t1', 'waiting_for_user_answer', 'inbox');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    cancelingThreadIds.value = new Set(['t1']);

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('canceling');
    if (state!.type === 'canceling') {
      expect(state!.isCanceling).toBe(true);
    }
  });
});

describe('External repo CC thread shows Done instead of lone Discard', () => {
  it('shows Done when isExternalRepo and proposed (Apply not available)', () => {
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    thread.meta.codingAgentChangeState = { kind: 'proposed', requires_restart: false };
    thread.meta.codingAgentIsExternalRepo = true;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.actions.map((a) => a.kind)).toContain('archive');
      expect(state!.actions.map((a) => a.kind)).not.toContain('discard');
      expect(state!.actions.map((a) => a.kind)).not.toContain('apply');
    }
  });

  it('shows Diff for an external repo whose branch holds unproposed work', () => {
    const thread = makeCCThread('t1', 'idle', 'inbox');
    thread.meta.codingAgentChangeState = { kind: 'unproposed', reason: null };
    thread.meta.codingAgentIsExternalRepo = true;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.showDiff).toBe(true);
    }
  });
});

describe('Diff button shows only when the CC thread has a diff', () => {
  it('internal CC thread with a proposal shows Diff even without a Change row', () => {
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    thread.meta.codingAgentChangeState = { kind: 'proposed', requires_restart: false };
    thread.meta.codingAgentIsExternalRepo = false;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.showDiff).toBe(true);
    }
  });

  it('internal CC thread with no changes hides Diff entirely', () => {
    const thread = makeCCThread('t1', 'idle', 'inbox');
    thread.meta.codingAgentChangeState = { kind: 'none' };
    thread.meta.codingAgentIsExternalRepo = false;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.showDiff).toBe(false);
    }
  });

  it('chat thread does not render the Diff button at all (no branch concept)', () => {
    const thread = makeCCThread('t1', 'idle', 'inbox');
    thread.meta.channel = 'chat';
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.actions.map((a) => a.kind)).toContain('archive');
      expect(state!.showDiff).toBe(false);
    }
  });
});

describe('Pending change with file_count=0 must not show Apply/Discard', () => {
  it('treats a zero-file pending change as no pending change (banner shows Done, not Apply/Discard)', () => {
    const thread = makeCCThread('t1', 'idle', 'inbox');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    changes.value = { status: 'loaded', data: [{
      id: 'c1',
      request_id: 'r1',
      thread_id: 't1',
      thread_title: null,
      branch_name: 'claude-code/empty-diff',
      repo_root: '/tmp/repo',
      description: 'phantom',
      file_count: 0,
      files: [],
      requires_restart: false,
      hardened: false,
      needs_hardening: true,
      apply_ready: false,
      status: 'pending',
      created_at: '',
      resolved_at: null,
      pre_merge_sha: null,
      post_merge_sha: null,
      commits: [],
      summary: null,
      incomplete: false,
    }] };

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      const actionKinds = state!.actions.map((a) => a.kind);
      expect(actionKinds).not.toContain('apply');
      expect(actionKinds).not.toContain('discard');
      expect(actionKinds).toContain('archive');
    }
  });

  it('still shows Apply/Discard for a real pending change with files', () => {
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    changes.value = { status: 'loaded', data: [{
      id: 'c1',
      request_id: 'r1',
      thread_id: 't1',
      thread_title: null,
      branch_name: 'claude-code/real',
      repo_root: '/tmp/repo',
      description: 'real change',
      file_count: 2,
      files: ['a.ts', 'b.ts'],
      requires_restart: false,
      hardened: false,
      needs_hardening: true,
      apply_ready: false,
      status: 'pending',
      created_at: '',
      resolved_at: null,
      pre_merge_sha: null,
      post_merge_sha: null,
      commits: [],
      summary: null,
      incomplete: false,
    }] };

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      const actionKinds = state!.actions.map((a) => a.kind);
      expect(actionKinds).toContain('apply');
      expect(actionKinds).toContain('discard');
    }
  });
});

describe('Apply* marker sources requires_restart from pending change', () => {
  it('shows requiresRestart when pending change has requires_restart=true even if meta says false', () => {
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    thread.meta.codingAgentChangeState = { kind: 'proposed', requires_restart: false };
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    changes.value = { status: 'loaded', data: [{
      id: 'c1',
      request_id: 'r1',
      thread_id: 't1',
      thread_title: null,
      branch_name: 'claude-code/restart',
      repo_root: '/tmp/repo',
      description: 'rust change',
      file_count: 1,
      files: ['crates/lucidos-engine/src/main.rs'],
      requires_restart: true,
      hardened: false,
      needs_hardening: true,
      apply_ready: false,
      status: 'pending',
      created_at: '',
      resolved_at: null,
      pre_merge_sha: null,
      post_merge_sha: null,
      commits: [],
      summary: null,
      incomplete: false,
    }] };

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      const apply = state!.actions.find((a) => a.kind === 'apply');
      expect(apply?.label).toBe('Apply*');
      expect(apply?.tooltip).toContain('new engine version');
    }
  });

  it('shows requiresRestart when meta says true even if no pending change row yet', () => {
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    thread.meta.codingAgentChangeState = { kind: 'proposed', requires_restart: true };
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      const apply = state!.actions.find((a) => a.kind === 'apply');
      expect(apply?.label).toBe('Apply*');
      expect(apply?.tooltip).toContain('new engine version');
    }
  });
});

describe('Apply Now — SessionEnded must not clear applying state', () => {
  it('getWaitingState returns applying even after thread status changes to waiting', () => {
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    thread.meta.codingAgentChangeState = { kind: 'proposed', requires_restart: false };
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    applyNowRequests.value = new Map([['t1', null]]);

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('applying');
  });

});

describe('merge-via-CC apply is cancelable (best-effort) from the thread', () => {
  it('returns canceling when status=running during a merge', () => {
    const thread = makeCCThread('t1', 'running', 'inbox');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('canceling');
  });

  it('returns canceling when status=waiting_for_user_answer during a merge', () => {
    const thread = makeCCThread('t1', 'waiting_for_user_answer', 'inbox');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('canceling');
  });
});
