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
  archiveThread: vi.fn().mockResolvedValue({ archived: [] }),
  saveThread: vi.fn(),
}));

vi.mock('../actions/thread-loading', () => ({
  loadThreadEvents: vi.fn(),
  sectionMutatedAt: new Map<string, number>(),
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
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
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
  archivingThreadIds.value = new Set();
  applyingNowThreadIds.value = new Map();
  discardingCCThreadIds.value = new Set();
  cancelingThreadIds.value = new Set();
  changes.value = { status: 'loaded', data: [] };
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
      // During archive the optimistic section flip empties the close set; the
      // disabled "Archive..." spinner is rendered from the isArchiving flag by
      // getBannerSlots, not from a selector action.
      expect(state!.actions).toEqual([]);
    }
  });

  it('returns null when thread is idle+default and NOT dismissing', () => {
    // Normal case: thread is in archive (idle + default), no dismiss in progress
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
      expect(state!.actions.map((a) => a.kind)).toContain('archive');
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
    thread.meta.codingAgentProposed = true;
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
    thread.meta.codingAgentProposed = true; // even with mid-turn work staged
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
  it('shows Done when isExternalRepo and proposed (Apply not available)', () => {
    // External repo: can't Apply (changes are in a different repo).
    // Must show Done, not a lone Discard button.
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    thread.meta.codingAgentProposed = true;
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

  it('shows Diff for external repo with branch diff even when codingAgentProposed is false', () => {
    // codingAgentProposed can drift to false while the branch is still ahead of main;
    // codingAgentHasDiff is the git-truth signal that survives that drift, so the
    // Diff button stays shown.
    const thread = makeCCThread('t1', 'idle', 'inbox');
    thread.meta.codingAgentProposed = false;
    thread.meta.codingAgentIsExternalRepo = true;
    thread.meta.codingAgentHasDiff = true;
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
  it('internal CC thread with codingAgentHasDiff=true shows Diff even without a Change row', () => {
    // CC made commits on the branch and the projection saw the branch is
    // ahead of main (codingAgentHasDiff=true), but the Change row hasn't
    // materialized yet. Diff is still shown — the git-truth signal carries
    // the affordance independently of the Change row's appearance.
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    thread.meta.codingAgentProposed = true;
    thread.meta.codingAgentIsExternalRepo = false;
    thread.meta.codingAgentHasDiff = true;
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
    // Archive-only banner on a CC thread that did no work. The Diff button is
    // not rendered at all — there is nothing to look at, so the affordance
    // would only mislead.
    const thread = makeCCThread('t1', 'idle', 'inbox');
    thread.meta.codingAgentProposed = false;
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
    // Regression: agent_recovery used to propose changes for branches with
    // commits but zero net diff (commit+revert pattern from CC's npm install
    // lockfile rename). The phantom change row had file_count=0 — Apply/Discard
    // would render but do nothing useful. Backend now refuses to create such
    // rows; this guards against any that already exist or sneak in via SSE.
    // Mirrors the real "Resolving iOS E2E Timeouts" state: idle + inbox.
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
      status: 'pending',
      created_at: '',
      resolved_at: null,
      pre_merge_sha: null,
      post_merge_sha: null,
      commits: [],
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
      status: 'pending',
      created_at: '',
      resolved_at: null,
      pre_merge_sha: null,
      post_merge_sha: null,
      commits: [],
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
  // Regression: WaitingBanner used to read requiresRestart only from
  // meta.codingAgentRequiresRestart (set by CodingAgentIdled). When a stale or fallback
  // CodingAgentIdled set that flag to false but the actual pending change had
  // requires_restart=true (e.g. recovery hardcoded false, or mid-iteration
  // transition), the button incorrectly showed a plain "Apply" instead of "Apply*".
  // The change row's own requires_restart is the authoritative file-derived value.
  it('shows requiresRestart when pending change has requires_restart=true even if meta says false', () => {
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    thread.meta.codingAgentProposed = true;
    thread.meta.codingAgentRequiresRestart = false; // stale meta
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
      requires_restart: true, // authoritative file-derived value
      hardened: false,
      status: 'pending',
      created_at: '',
      resolved_at: null,
      pre_merge_sha: null,
      post_merge_sha: null,
      commits: [],
      incomplete: false,
    }] };

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('actions');
    if (state!.type === 'actions') {
      // A restart-requiring change gets the compact "Apply*" marker (restart is
      // still the separate switch); requiresRestart also surfaces via the Apply
      // TaggedAction's tooltip.
      const apply = state!.actions.find((a) => a.kind === 'apply');
      expect(apply?.label).toBe('Apply*');
      expect(apply?.tooltip).toContain('new engine version');
    }
  });

  it('shows requiresRestart when meta says true even if no pending change row yet', () => {
    // Symmetric: meta-only signal still works (e.g. before SSE delivers the
    // changes-updated broadcast).
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    thread.meta.codingAgentProposed = true;
    thread.meta.codingAgentRequiresRestart = true;
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
    // Scenario: during Apply Now, backend kills Claude Code session (SessionEnded → status=waiting)
    // THEN proposes the change (ChangeProposed). Between those two events,
    // applyingNowThreadIds must stay set so the banner keeps showing "Applying...".
    const thread = makeCCThread('t1', 'waiting', 'inbox');
    thread.meta.codingAgentProposed = true;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';
    applyingNowThreadIds.value = new Map([['t1', 'requesting']]);

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('applying');
    // Must NOT fall through to resolveActions which would return ['discard', 'apply']
  });

});

describe('merge-via-CC apply is cancelable (best-effort) from the thread', () => {
  it('returns canceling when status=running and codingAgentApplying=true', () => {
    // Scenario: user clicked Apply on the Changes panel for a thread with a
    // live Claude Code session. Tier 1 slow-path emits MergeConflictDetected (sets
    // codingAgentApplying=true), then sends a merge prompt to CC. CC processes the
    // prompt → CodingAgentPromptSent flips status to 'running'.
    //
    // The user must be able to stop a long-running merge: Cancel interrupts the
    // CC merge session. It's best-effort — if the merge already landed before the
    // interrupt processes, the engine still emits ChangeApplied; otherwise the
    // change returns to pending. (This used to be suppressed to a disabled
    // "Apply...", leaving no way out of a stuck merge.)
    const thread = makeCCThread('t1', 'running', 'inbox');
    thread.meta.codingAgentApplying = true;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('canceling');
  });

  it('returns canceling when status=waiting_for_user_answer and codingAgentApplying=true', () => {
    // Symmetric: a merge paused on a CC question is still mid-turn and cancelable.
    const thread = makeCCThread('t1', 'waiting_for_user_answer', 'inbox');
    thread.meta.codingAgentApplying = true;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('canceling');
  });

  it('still shows Cancel for normal CC running with codingAgentApplying=false', () => {
    // Regression guard: ordinary CC turns are cancelable too.
    const thread = makeCCThread('t1', 'running', 'inbox');
    thread.meta.codingAgentApplying = false;
    threadMap.value = new Map([['t1', thread]]);
    focusedThreadId.value = 't1';

    const state = getWaitingState();
    expect(state).not.toBeNull();
    expect(state!.type).toBe('canceling');
  });
});
