/**
 * Tests for the dismiss flow and mutual exclusivity of applying/dismissing states.
 *
 * Regression: clicking "Done" on an idle CC thread must not flash through a gap
 * where the WaitingBanner disappears. getWaitingState() checks dismissingThreadIds
 * before resolveActions, keeping the "Done..." button visible throughout dismiss.
 *
 * Invariant: applyingNowThreadIds, dismissingThreadIds, and discardingCCThreadIds
 * must never contain the same thread. Each action guards against the others.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  threadMap,
  focusedThreadId,
  dismissingThreadIds,
  applyingNowThreadIds,
  discardingCCThreadIds,
  changes,
} from '../store';
import type { ThreadState } from '../thread-events';
import { getWaitingState } from '../../components/chat/WaitingBanner';

// Mocks for handleDismissThread dependencies
vi.mock('../../api/threads', () => ({
  dismissThread: vi.fn().mockResolvedValue(undefined),
  pinThread: vi.fn(),
  unpinThread: vi.fn(),
}));

vi.mock('../actions/thread-loading', () => ({
  loadThreadEvents: vi.fn(),
}));

vi.mock('../../components/chat/scrollState', () => ({
  scrollToBottom: vi.fn(),
  notAtTop: { value: false },
}));

vi.mock('../../components/chat/promptFocus', () => ({
  focusPromptNow: vi.fn(),
  focusIfNeeded: vi.fn(),
}));

import { handleDismissThread } from '../actions/threads';

function makeCCThread(id: string, status: 'idle' | 'running' | 'waiting' | 'waiting_for_user_answer', section: 'default' | 'unread'): ThreadState {
  return {
    meta: {
      id,
      title: 'test',
      channel: 'claude_code',
      initiator: 'user',
      pinned: false,
      createdAt: '',
      updatedAt: '',
      unread: section === 'unread',
      status,
      messageCount: 0,
      section,
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      lastRevivedAt: '',
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
  dismissingThreadIds.value = new Set();
  applyingNowThreadIds.value = new Map();
  discardingCCThreadIds.value = new Set();
  changes.value = [];
});

describe('Done dismiss does not flash Requesting state', () => {
  it('returns dismiss-in-progress state even after SSE changes status to idle', () => {
    // Setup: CC thread is idle (ThreadDismissed SSE already arrived) but dismiss API
    // hasn't returned yet — dismissingThreadIds still has the thread.
    const thread = makeCCThread('t1', 'idle', 'default');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    dismissingThreadIds.value = new Set(['t1']);

    const state = getWaitingState();

    // Must NOT return null — that would cause the banner to disappear
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.isDismissing).toBe(true);
      expect(state!.actions).toContain('done');
    }
  });

  it('returns null when thread is idle+default and NOT dismissing', () => {
    // Normal case: thread is in history (idle + default), no dismiss in progress
    const thread = makeCCThread('t1', 'idle', 'default');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).toBeNull();
  });

  it('returns actions when thread is waiting+unread and NOT dismissing', () => {
    // Normal case: CC idle thread waiting for user action
    const thread = makeCCThread('t1', 'waiting', 'unread');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.actions).toContain('done');
      expect(state!.isDismissing).toBe(false);
    }
  });

  it('dismiss clears stale applying state — states are mutually exclusive', async () => {
    // Scenario: applyingNowThreadIds has thread (e.g., from a stale apply attempt)
    // and user clicks Done. handleDismissThread must clear applying state before
    // setting dismissing state — the two must never coexist for the same thread.
    const thread = makeCCThread('t1', 'waiting', 'unread');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    applyingNowThreadIds.value = new Map([['t1', 'requesting']]);

    await handleDismissThread('t1');

    // Applying state must have been cleared — states are mutually exclusive
    expect(applyingNowThreadIds.value.has('t1')).toBe(false);
    // Dismiss completed and cleaned up
    expect(dismissingThreadIds.value.has('t1')).toBe(false);
  });
});

describe('Apply button never shows Requesting label', () => {
  it('returns applying state (always renders "Apply...")', () => {
    const thread = makeCCThread('t1', 'waiting', 'unread');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    applyingNowThreadIds.value = new Map([['t1', 'requesting']]);

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('applying');
  });
});

describe('Discard hides Apply and shows "Discard..."', () => {
  it('returns discarding state when discardingCCThreadIds is set', () => {
    const thread = makeCCThread('t1', 'waiting', 'unread');
    thread.meta.ccHasChanges = true;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    discardingCCThreadIds.value = new Set(['t1']);

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('discarding');
  });

  it('discarding takes priority over action buttons', () => {
    // Even though resolveActions would return ['discard', 'apply'],
    // the discarding state should preempt them.
    const thread = makeCCThread('t1', 'waiting', 'unread');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    discardingCCThreadIds.value = new Set(['t1']);

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('discarding');
  });

  it('applying takes priority over discarding (should not coexist)', () => {
    const thread = makeCCThread('t1', 'waiting', 'unread');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    applyingNowThreadIds.value = new Map([['t1', 'requesting']]);
    discardingCCThreadIds.value = new Set(['t1']);

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('applying');
  });
});

describe('WaitingForUserAnswer surfaces Done so the user can abandon the question', () => {
  it('shows Done (only) when CC is paused on AskUserQuestion', () => {
    // Bug regression: WaitingBanner returned null for waiting_for_user_answer,
    // leaving the user with no way to dismiss the thread when they wanted to
    // abandon the question. Done must be available; Apply/Discard must NOT
    // (the mid-turn work is incomplete).
    const thread = makeCCThread('t1', 'waiting_for_user_answer', 'unread');
    thread.meta.ccHasChanges = true; // even with mid-turn work staged
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.actions).toEqual(['done']);
    }
  });
});

describe('External repo CC thread shows Done instead of lone Discard', () => {
  it('shows Done when isExternalRepo and hasChanges (Apply not available)', () => {
    // External repo: can't Apply (changes are in a different repo).
    // Must show Done, not a lone Discard button.
    const thread = makeCCThread('t1', 'waiting', 'unread');
    thread.meta.ccHasChanges = true;
    thread.meta.ccIsExternalRepo = true;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.actions).toContain('done');
      expect(state!.actions).not.toContain('discard');
      expect(state!.actions).not.toContain('apply');
    }
  });
});

describe('Pending change with file_count=0 must not show Apply/Discard', () => {
  it('treats a zero-file pending change as no pending change (banner shows Done, not Apply/Discard)', () => {
    // Regression: agent_recovery used to propose changes for branches with
    // commits but zero net diff (commit+revert pattern from CC's npm install
    // lockfile rename). The phantom change row had file_count=0 — Apply/Discard
    // would render but do nothing useful. Backend now refuses to create such
    // rows; this guards against any that already exist or sneak in via SSE.
    // Mirrors the real "Resolving iOS E2E Timeouts" state: idle + unread.
    const thread = makeCCThread('t1', 'idle', 'unread');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    changes.value = [{
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
      status: 'pending',
      created_at: '',
      resolved_at: null,
      pre_merge_sha: null,
      post_merge_sha: null,
      commits: [],
    }];

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.actions).not.toContain('apply');
      expect(state!.actions).not.toContain('discard');
      expect(state!.actions).toContain('done');
      expect(state!.pendingChange).toBeNull();
    }
  });

  it('still shows Apply/Discard for a real pending change with files', () => {
    const thread = makeCCThread('t1', 'waiting', 'unread');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    changes.value = [{
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
      status: 'pending',
      created_at: '',
      resolved_at: null,
      pre_merge_sha: null,
      post_merge_sha: null,
      commits: [],
    }];

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.actions).toContain('apply');
      expect(state!.actions).toContain('discard');
      expect(state!.pendingChange).not.toBeNull();
    }
  });
});

describe('Apply & Restart label sources requires_restart from pending change', () => {
  // Regression: WaitingBanner used to read requiresRestart only from
  // meta.ccRequiresRestart (set by CodingAgentIdled). When a stale or fallback
  // CodingAgentIdled set that flag to false but the actual pending change had
  // requires_restart=true (e.g. recovery hardcoded false, or mid-iteration
  // transition), the button incorrectly showed "Apply" instead of "Apply & Restart".
  // The change row's own requires_restart is the authoritative file-derived value.
  it('shows requiresRestart when pending change has requires_restart=true even if meta says false', () => {
    const thread = makeCCThread('t1', 'waiting', 'unread');
    thread.meta.ccHasChanges = true;
    thread.meta.ccRequiresRestart = false; // stale meta
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    changes.value = [{
      id: 'c1',
      request_id: 'r1',
      thread_id: 't1',
      thread_title: null,
      branch_name: 'claude-code/restart',
      repo_root: '/tmp/repo',
      description: 'rust change',
      file_count: 1,
      files: ['crates/cognos-engine/src/main.rs'],
      requires_restart: true, // authoritative file-derived value
      hardened: false,
      status: 'pending',
      created_at: '',
      resolved_at: null,
      pre_merge_sha: null,
      post_merge_sha: null,
      commits: [],
    }];

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.requiresRestart).toBe(true);
    }
  });

  it('shows requiresRestart when meta says true even if no pending change row yet', () => {
    // Symmetric: meta-only signal still works (e.g. before SSE delivers the
    // changes-updated broadcast).
    const thread = makeCCThread('t1', 'waiting', 'unread');
    thread.meta.ccHasChanges = true;
    thread.meta.ccRequiresRestart = true;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.requiresRestart).toBe(true);
    }
  });
});

describe('Apply Now — SessionEnded must not clear applying state', () => {
  it('getWaitingState returns applying even after thread status changes to waiting', () => {
    // Scenario: during Apply Now, backend kills CC session (SessionEnded → status=waiting)
    // THEN proposes the change (ChangeProposed). Between those two events,
    // applyingNowThreadIds must stay set so the banner keeps showing "Applying...".
    const thread = makeCCThread('t1', 'waiting', 'unread');
    thread.meta.ccHasChanges = true;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    applyingNowThreadIds.value = new Map([['t1', 'requesting']]);

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('applying');
    // Must NOT fall through to resolveActions which would return ['discard', 'apply']
  });

});
