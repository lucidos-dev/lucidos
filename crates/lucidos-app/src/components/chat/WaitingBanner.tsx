import { threadMap, focusedThreadId, applyingNowThreadIds, archivingThreadIds, discardingCCThreadIds, cancelingThreadIds, changes, showConfirm, effectiveThreadStatus, isMidTurn } from '../../store/store';
import { getCCWaitingInfo } from '../../store/thread-events';
import { resolveActions, type Action } from '../../generated/thread-lifecycle';
import { endClaudeCodeAndApply, handleDiscardCCChanges } from '../../store/actions/chat-claude-code';
import { handleArchiveThread } from '../../store/actions/threads';
import { viewChangeDiff, viewThreadCcDiff } from '../../store/actions/repositories';
import type { Change } from '../../api/client';

// Action buttons: Archive, Apply, Discard — never "Requesting"
const ARCHIVE_ACTIONS: Action[] = ['archive'];

export type WaitingState =
  | { type: 'applying' }
  | { type: 'discarding' }
  | { type: 'canceling'; threadId: string; isCanceling: boolean }
  | { type: 'actions'; actions: Action[]; threadId: string; isArchiving: boolean; requiresRestart: boolean; incomplete: boolean; pendingChange: Change | null; externalCcDiffAvailable: boolean };

/** WaitingBanner renders Apply / Discard / Archive / Diff buttons. The
 *  'canceling' variant is owned by PromptInput's morphable Send→Cancel
 *  button (so the swap can animate the same DOM node) and must never be
 *  passed here — narrow it out at the call site. */
export type BannerState = Exclude<WaitingState, { type: 'canceling' }>;

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
    return { type: 'actions', actions: ARCHIVE_ACTIONS, threadId: focused, isArchiving: true, requiresRestart: false, incomplete: false, pendingChange: null, externalCcDiffAvailable: false };
  }

  const status = effectiveThreadStatus(thread);

  // Mid-turn states get Cancel. Must come before resolveActions, which returns
  // [] for both and would otherwise drop us into the "no banner" branch.
  // Excludes 'waiting' (CC has changes — needs Apply/Discard, not Cancel).
  if (isMidTurn(status)) {
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
  let actions = resolveActions(threadType, status, thread.meta.section, hasPendingChanges, thread.meta.saved);
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

  // External-repo CC sessions never produce a `Change` row, and ccHasChanges
  // can drift to false while the worktree branch is still ahead of main. The
  // Diff button compares branch vs main on demand, so always offer it here.
  const externalCcDiffAvailable =
    threadType === 'claude_code' && thread.meta.ccIsExternalRepo;

  // Surface partial-work warnings for changes proposed from a CC turn that
  // ended in `ResponseFailed` (mid-stream API drop, etc.). The Apply button
  // confirms before landing so the user can't accidentally merge half a turn.
  const incomplete = pendingChange?.incomplete ?? false;

  return { type: 'actions', actions, threadId: focused, isArchiving: false, requiresRestart, incomplete, pendingChange, externalCcDiffAvailable };
}

export function WaitingBanner({ state }: { state: BannerState }) {
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

  return (
    <div class="thread-action-buttons">
      {state.pendingChange && (
        <button class="action-btn" onClick={() => viewChangeDiff(state.pendingChange!)}>Diff</button>
      )}
      {!state.pendingChange && state.externalCcDiffAvailable && (
        <button class="action-btn" onClick={() => viewThreadCcDiff(state.threadId)}>Diff</button>
      )}
      {state.actions.map(action => renderActionButton(action, state.threadId, state.isArchiving, state.requiresRestart, state.incomplete))}
    </div>
  );
}

function renderActionButton(action: Action, threadId: string, isArchiving: boolean, requiresRestart = false, incomplete = false) {
  switch (action) {
    case 'archive':
      return (
        <button class="action-btn" disabled={isArchiving}
          onClick={() => handleArchiveThread(threadId)}>
          {isArchiving ? 'Archive...' : 'Archive'}
        </button>
      );
    case 'apply': {
      // Tooltip prefers the partial-work warning (more critical) over the
      // restart hint when both apply.
      const tooltip = incomplete
        ? 'This change was proposed by a turn that ended in failure. The worktree contents may be partial work. You will be asked to confirm.'
        : requiresRestart
          ? 'Engine restart required for these changes to be applied correctly. You will be prompted to restart'
          : undefined;
      const onClick = async () => {
        if (incomplete) {
          const ok = await showConfirm(
            'This change was proposed by a turn that ended in failure (e.g. mid-stream API drop). The worktree contents may be incomplete. Apply anyway?',
            'Apply',
          );
          if (!ok) return;
        }
        endClaudeCodeAndApply(threadId);
      };
      return (
        <button class="action-btn action-btn-confirm"
          data-tooltip={tooltip}
          onClick={onClick}>
          {requiresRestart ? 'Apply & Restart' : 'Apply'}
        </button>
      );
    }
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
