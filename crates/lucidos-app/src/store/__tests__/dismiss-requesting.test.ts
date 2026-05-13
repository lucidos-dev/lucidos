/**
 * Tests for the dismiss flow and mutual exclusivity of applying/dismissing states.
 *
 * Regression: clicking "Archive" on an idle CC thread must not flash through a gap
 * where the WaitingBanner disappears. getWaitingState() checks archivingThreadIds
 * before resolveActions, keeping the "Archive..." button visible throughout dismiss.
 *
 * Invariant: applyingNowThreadIds, archivingThreadIds, and discardingCCThreadIds
 * must never contain the same thread. Each action guards against the others.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  threadMap,
  focusedThreadId,
  archivingThreadIds,
  applyingNowThreadIds,
  discardingCCThreadIds,
  cancelingThreadIds,
  changes,
} from '../store';
import type { ThreadState } from '../thread-events';
import { getWaitingState } from '../../components/chat/WaitingBanner';

// Mocks for handleArchiveThread dependencies
vi.mock('../../api/threads', () => ({
  archiveThread: vi.fn().mockResolvedValue(undefined),
  saveThread: vi.fn(),
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

import { handleArchiveThread } from '../actions/threads';

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
      messageCount: 0,
      section,
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      ccHasChanges: false,
      ccRequiresRestart: false,
      ccIsExternalRepo: false,
      ccApplying: false,
      lastRevivedAt: '',
      state: 'active',
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
  archivingThreadIds.value = new Set();
  applyingNowThreadIds.value = new Map();
  discardingCCThreadIds.value = new Set();
  cancelingThreadIds.value = new Set();
  changes.value = [];
});

describe('Done dismiss does not flash Requesting state', () => {
  it('returns dismiss-in-progress state even after SSE changes status to idle', () => {
    // Setup: CC thread is idle (ThreadDismissed SSE already arrived) but dismiss API
    // hasn't returned yet — archivingThreadIds still has the thread.
    const thread = makeCCThread('t1', 'idle', 'archived');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    archivingThreadIds.value = new Set(['t1']);

    const state = getWaitingState();

    // Must NOT return null — that would cause the banner to disappear
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.isArchiving).toBe(true);
      expect(state!.actions).toContain('archive');
    }
  });

  it('returns null when thread is idle+default and NOT dismissing', () => {
    // Normal case: thread is in history (idle + default), no dismiss in progress
    const thread = makeCCThread('t1', 'idle', 'archived');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).toBeNull();
  });

  it('returns actions when thread is waiting+inbox and NOT dismissing', () => {
    // Normal case: CC idle thread waiting for user action
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.actions).toContain('archive');
      expect(state!.isArchiving).toBe(false);
    }
  });

  it('dismiss clears stale applying state — states are mutually exclusive', async () => {
    // Scenario: applyingNowThreadIds has thread (e.g., from a stale apply attempt)
    // and user clicks Done. handleArchiveThread must clear applying state before
    // setting dismissing state — the two must never coexist for the same thread.
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    applyingNowThreadIds.value = new Map([['t1', 'requesting']]);

    await handleArchiveThread('t1');

    // Applying state must have been cleared — states are mutually exclusive
    expect(applyingNowThreadIds.value.has('t1')).toBe(false);
    // Dismiss completed and cleaned up
    expect(archivingThreadIds.value.has('t1')).toBe(false);
  });
});

describe('Apply button never shows Requesting label', () => {
  it('returns applying state (always renders "Apply...")', () => {
    const thread = makeCCThread('t1', 'waiting', 'inbox');
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
    const thread = makeCCThread('t1', 'waiting', 'inbox');
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
    applyingNowThreadIds.value = new Map([['t1', 'requesting']]);
    discardingCCThreadIds.value = new Set(['t1']);

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('applying');
  });
});

describe('WaitingForUserAnswer surfaces Cancel so the user can abandon the question', () => {
  it('shows Cancel (only) when CC is paused on AskUserQuestion', () => {
    // Save/Archive must NOT show while the question is still pending —
    // mid-turn work hasn't terminated yet.
    const thread = makeCCThread('t1', 'waiting_for_user_answer', 'inbox');
    thread.meta.ccHasChanges = true; // even with mid-turn work staged
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
    // After the user clicks Cancel, handleCancelExchange adds the thread to
    // cancelingThreadIds before any SSE arrives. Status is still
    // waiting_for_user_answer. The banner must show "Cancel..." (disabled) so
    // the user can't double-fire — and the PromptInput cleanup effect must NOT
    // clear the flag while we're still in this status.
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
  it('shows Done when isExternalRepo and hasChanges (Apply not available)', () => {
    // External repo: can't Apply (changes are in a different repo).
    // Must show Done, not a lone Discard button.
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    thread.meta.ccHasChanges = true;
    thread.meta.ccIsExternalRepo = true;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.actions).toContain('archive');
      expect(state!.actions).not.toContain('discard');
      expect(state!.actions).not.toContain('apply');
    }
  });

  it('exposes Diff (enabled) for external repo even when ccHasChanges is false', () => {
    // ccHasChanges can drift to false while the branch is still ahead of main;
    // for external-repo CC threads the Diff button must stay enabled regardless.
    const thread = makeCCThread('t1', 'idle', 'inbox');
    thread.meta.ccHasChanges = false;
    thread.meta.ccIsExternalRepo = true;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.ccDiff).toBe('enabled');
    }
  });
});

describe('Diff button is always offered for CC threads, disabled when there is no diff', () => {
  it('internal CC thread with ccHasChanges=true shows Diff enabled even without a Change row', () => {
    // The user's reported case: CC made changes, ChangeProposed has fired
    // (cc_has_changes=true) but the Change row hasn't materialized yet.
    // Pre-fix the Diff button only showed when pendingChange existed.
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    thread.meta.ccHasChanges = true;
    thread.meta.ccIsExternalRepo = false;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.pendingChange).toBeNull();
      expect(state!.ccDiff).toBe('enabled');
    }
  });

  it('internal CC thread with no changes shows Diff disabled', () => {
    // Archive-only banner on a CC thread that did no work. The Diff button
    // still renders so the user sees the affordance, but it's disabled
    // (with a tooltip) since there's nothing to look at.
    const thread = makeCCThread('t1', 'idle', 'inbox');
    thread.meta.ccHasChanges = false;
    thread.meta.ccIsExternalRepo = false;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.ccDiff).toBe('disabled');
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
      expect(state!.actions).toContain('archive');
      expect(state!.ccDiff).toBe('hidden');
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
    // Mirrors the real "Resolving iOS E2E Timeouts" state: idle + inbox.
    const thread = makeCCThread('t1', 'idle', 'inbox');
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
      incomplete: false,
    }];

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      expect(state!.actions).not.toContain('apply');
      expect(state!.actions).not.toContain('discard');
      expect(state!.actions).toContain('archive');
      expect(state!.pendingChange).toBeNull();
    }
  });

  it('still shows Apply/Discard for a real pending change with files', () => {
    const thread = makeCCThread('t1', 'waiting', 'inbox');
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
      incomplete: false,
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
    const thread = makeCCThread('t1', 'waiting', 'inbox');
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
      files: ['crates/lucidos-engine/src/main.rs'],
      requires_restart: true, // authoritative file-derived value
      hardened: false,
      status: 'pending',
      created_at: '',
      resolved_at: null,
      pre_merge_sha: null,
      post_merge_sha: null,
      commits: [],
      incomplete: false,
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
    const thread = makeCCThread('t1', 'waiting', 'inbox');
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
    const thread = makeCCThread('t1', 'waiting', 'inbox');
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

describe('ccApplying suppresses Cancel button during merge-via-CC apply', () => {
  it('returns applying (not canceling) when status=running and ccApplying=true', () => {
    // Scenario: user clicked Apply on the Changes panel for a thread with a
    // live CC session. Tier 1 slow-path emits MergeConflictDetected (sets
    // ccApplying=true), then sends a merge prompt to CC. CC processes the
    // prompt → CodingAgentPromptSent flips status to 'running'.
    //
    // Without this guard, the WaitingBanner would show the Cancel button.
    // Clicking Cancel only interrupts the CC subprocess — the apply task in
    // apply_change keeps running, sees CC went idle, checks if main is now
    // an ancestor of the branch, and emits ChangeApplied anyway. The user
    // thinks they cancelled but the merge lands. Hide Cancel during apply
    // so the user can't trigger a no-op cancel.
    const thread = makeCCThread('t1', 'running', 'inbox');
    thread.meta.ccApplying = true;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('applying');
  });

  it('returns applying when status=waiting_for_user_answer and ccApplying=true', () => {
    // Symmetric: even if CC pauses on a question mid-merge, suppress Cancel.
    // The apply task is still in flight in the engine.
    const thread = makeCCThread('t1', 'waiting_for_user_answer', 'inbox');
    thread.meta.ccApplying = true;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('applying');
  });

  it('still shows Cancel for normal CC running with ccApplying=false', () => {
    // Regression guard: don't suppress Cancel for ordinary CC turns where no
    // apply is in flight. ccApplying must be the sole gate.
    const thread = makeCCThread('t1', 'running', 'inbox');
    thread.meta.ccApplying = false;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('canceling');
  });
});
