import { threadMap, focusedThreadId, applyingNowThreadIds, archivingThreadIds, discardingCCThreadIds, cancelingThreadIds, changes, showConfirm, effectiveThreadStatus } from '../../store/store';
import { getCCWaitingInfo } from '../../store/thread-events';
import { resolveActions, type Action } from '../../generated/thread-lifecycle';
import { endClaudeCodeAndApply, handleDiscardCCChanges } from '../../store/actions/chat-claude-code';
import { handleArchiveThread } from '../../store/actions/threads';
import { handleCancelExchange } from '../../store/actions/chat';
import { viewChangeDiff, viewThreadCcDiff } from '../../store/actions/repositories';
import type { Change } from '../../api/client';

// Action buttons: Archive, Apply, Discard — never "Requesting"
const ARCHIVE_ACTIONS: Action[] = ['archive'];

export type WaitingState =
  | { type: 'applying' }
  | { type: 'discarding' }
  | { type: 'canceling'; threadId: string; isCanceling: boolean }
  | { type: 'actions'; actions: Action[]; threadId: string; isArchiving: boolean; requiresRestart: boolean; pendingChange: Change | null; externalCcDiffAvailable: boolean };

export function getWaitingState(): WaitingState | null {
  const focused = focusedThreadId.value;
  if (!focused) return null;

  const thread = threadMap.value.get(focused);
  if (!thread) return null;

  // Applying in progress — show "Apply..." and block all other actions.
  // The Archive button must never render while apply is active. The actions
  // (handleArchiveThread / endClaudeCodeAndApply) enforce mutual exclusivity,
  // so applying, dismissing, and discarding can't coexist for the same thread.
  if (applyingNowThreadIds.value.has(focused)) return { type: 'applying' };

  // Discarding in progress — show "Discard..." and block all other actions.
  if (discardingCCThreadIds.value.has(focused)) return { type: 'discarding' };

  // Archive in progress — keep showing "Archive..." regardless of SSE state
  // changes so the banner doesn't flash away mid-archive.
  const isArchiving = archivingThreadIds.value.has(focused);
  if (isArchiving) {
    return { type: 'actions', actions: ARCHIVE_ACTIONS, threadId: focused, isArchiving: true, requiresRestart: false, pendingChange: null, externalCcDiffAvailable: false };
  }

  const status = effectiveThreadStatus(thread);

  // Mid-turn states get Cancel. Must come before resolveActions, which returns
  // [] for both and would otherwise drop us into the "no banner" branch.
  // Excludes 'waiting' (CC has changes — needs Apply/Discard, not Cancel).
  if (status === 'running' || status === 'waiting_for_user_answer') {
    // ccApplying = MergeConflictDetected fired and the apply task is driving
    // the CC session through a merge. The 'running' status reflects that
    // engine-pushed merge prompt, not a user turn. Cancel here would only
    // interrupt CC mid-merge — the apply task in the engine continues, sees
    // CC went idle, and emits ChangeApplied if the merge had already landed.
    // Show "Apply..." instead so the user can't trigger a no-op cancel.
    if (thread.meta.ccApplying) return { type: 'applying' };
    return {
      type: 'canceling',
      threadId: focused,
      isCanceling: cancelingThreadIds.value.has(focused),
    };
  }

  const threadType = thread.meta.channel === 'claude_code' ? 'claude_code' as const : 'chat' as const;
  const ccInfo = threadType === 'claude_code' ? getCCWaitingInfo(thread.meta) : null;
  // Use ccInfo.hasChanges as an early signal — CodingAgentIdled arrives before
  // ChangeProposed (which requires async git ops + DB insert). Without this,
  // there's a window where the banner flashes "Archive" before switching to Apply/Discard.
  // file_count=0 rows are phantom changes (commit + revert with zero net diff)
  // and must not surface Apply/Discard — there's nothing to apply or discard.
  const pendingChange = changes.value.find(
    c => c.thread_id === focused && c.status === 'pending' && c.file_count > 0,
  ) ?? null;
  const hasPendingChanges = !!pendingChange || (ccInfo?.hasChanges ?? false);
  let actions = resolveActions(threadType, status, thread.meta.section, hasPendingChanges);
  if (actions.length === 0) return null;

  let requiresRestart = false;
  if (threadType === 'claude_code' && actions.includes('apply')) {
    if (ccInfo?.isExternalRepo) {
      // External repo: can't Apply (changes are in a different repo).
      // Show Archive instead so the user can dismiss the thread.
      actions = ARCHIVE_ACTIONS;
    } else if (pendingChange?.requires_restart || ccInfo?.requiresRestart) {
      // Prefer the pending change's flag — it's the authoritative file-derived
      // value at proposal time. meta.ccRequiresRestart can lag (only set by
      // CodingAgentIdled) or be wrong (recovery fallback hardcodes false).
      requiresRestart = true;
    }
  }

  // External-repo CC sessions never produce a `Change` row but the user still
  // wants to see what CC changed in the worktree branch. Surface a Diff button
  // whenever CC has reported changes in such a session.
  const externalCcDiffAvailable = !!ccInfo?.isExternalRepo && (ccInfo?.hasChanges ?? false);

  return { type: 'actions', actions, threadId: focused, isArchiving: false, requiresRestart, pendingChange, externalCcDiffAvailable };
}

export function WaitingBanner({ state }: { state: WaitingState }) {
  if (state.type === 'applying') {
    return (
      <div class="thread-action-buttons">
        <button class="action-btn action-btn-confirm" disabled>Apply...</button>
      </div>
    );
  }

  if (state.type === 'discarding') {
    return (
      <div class="thread-action-buttons">
        <button class="action-btn action-btn-danger" disabled>Discard...</button>
      </div>
    );
  }

  if (state.type === 'canceling') {
    return (
      <div class="thread-action-buttons">
        <button class="action-btn action-btn-danger" disabled={state.isCanceling}
          onClick={() => handleCancelExchange(state.threadId)}>
          {state.isCanceling ? 'Cancel...' : 'Cancel'}
        </button>
      </div>
    );
  }

  return (
    <div class="thread-action-buttons">
      {state.pendingChange && (
        <button class="action-btn" onClick={() => viewChangeDiff(state.pendingChange!)}>Diff</button>
      )}
      {!state.pendingChange && state.externalCcDiffAvailable && (
        <button class="action-btn" onClick={() => viewThreadCcDiff(state.threadId)}>Diff</button>
      )}
      {state.actions.map(action => renderActionButton(action, state.threadId, state.isArchiving, state.requiresRestart))}
    </div>
  );
}

function renderActionButton(action: Action, threadId: string, isArchiving: boolean, requiresRestart = false) {
  switch (action) {
    case 'archive':
      return (
        <button class="action-btn" disabled={isArchiving}
          onClick={() => handleArchiveThread(threadId)}>
          {isArchiving ? 'Archive...' : 'Archive'}
        </button>
      );
    case 'apply':
      return (
        <button class="action-btn action-btn-confirm"
          data-tooltip={requiresRestart ? 'Engine restart required for these changes to be applied correctly. You will be prompted to restart' : undefined}
          onClick={() => endClaudeCodeAndApply(threadId)}>
          {requiresRestart ? 'Apply & Restart' : 'Apply'}
        </button>
      );
    case 'discard':
      return (
        <button class="action-btn action-btn-danger"
          onClick={async () => {
            if (await showConfirm('Discard all changes from this session? This cannot be undone.', 'Discard')) {
              handleDiscardCCChanges(threadId);
            }
          }}>
          Discard
        </button>
      );
  }
}
