import { threadMap, focusedThreadId, applyingNowThreadIds, dismissingThreadIds, discardingCCThreadIds, changes, showConfirm, effectiveThreadStatus } from '../../store/store';
import { getCCWaitingInfo } from '../../store/thread-events';
import { resolveActions, type Action } from '../../generated/thread-lifecycle';
import { endClaudeCodeAndApply, handleDiscardCCChanges } from '../../store/actions/chat-claude-code';
import { handleDismissThread } from '../../store/actions/threads';
import { viewChangeDiff } from '../../store/actions/repositories';
import type { Change } from '../../api/client';

// Action buttons: Done, Apply, Discard — never "Requesting"
const DISMISS_ACTIONS: Action[] = ['done'];

export type WaitingState =
  | { type: 'applying' }
  | { type: 'discarding' }
  | { type: 'actions'; actions: Action[]; threadId: string; isDismissing: boolean; requiresRestart: boolean; pendingChange: Change | null };

export function getWaitingState(): WaitingState | null {
  const focused = focusedThreadId.value;
  if (!focused) return null;

  const thread = threadMap.value.get(focused);
  if (!thread) return null;

  // Applying in progress — show "Apply..." and block all other actions.
  // The Done button must never render while apply is active. The actions
  // (handleDismissThread / endClaudeCodeAndApply) enforce mutual exclusivity,
  // so applying, dismissing, and discarding can't coexist for the same thread.
  if (applyingNowThreadIds.value.has(focused)) return { type: 'applying' };

  // Discarding in progress — show "Discard..." and block all other actions.
  if (discardingCCThreadIds.value.has(focused)) return { type: 'discarding' };

  // Dismiss in progress — keep showing "Done..." regardless of SSE state
  // changes so the banner doesn't flash away mid-dismiss.
  const isDismissing = dismissingThreadIds.value.has(focused);
  if (isDismissing) {
    return { type: 'actions', actions: DISMISS_ACTIONS, threadId: focused, isDismissing: true, requiresRestart: false, pendingChange: null };
  }

  const status = effectiveThreadStatus(thread);
  const threadType = thread.meta.channel === 'claude_code' ? 'claude_code' as const : 'chat' as const;
  const ccInfo = threadType === 'claude_code' ? getCCWaitingInfo(thread.meta) : null;
  // Use ccInfo.hasChanges as an early signal — CodingAgentIdled arrives before
  // ChangeProposed (which requires async git ops + DB insert). Without this,
  // there's a window where the banner flashes "Done" before switching to Apply/Discard.
  // file_count=0 rows are phantom changes (commit + revert with zero net diff)
  // and must not surface Apply/Discard — there's nothing to apply or discard.
  const pendingChange = changes.value.find(
    c => c.thread_id === focused && c.status === 'pending' && c.file_count > 0,
  ) ?? null;
  const hasPendingChanges = !!pendingChange || (ccInfo?.hasChanges ?? false);
  const storedSection = thread.meta.section as 'default' | 'unread';

  let actions = resolveActions(threadType, status, storedSection, hasPendingChanges);
  if (actions.length === 0) return null;

  let requiresRestart = false;
  if (threadType === 'claude_code' && actions.includes('apply')) {
    if (ccInfo?.isExternalRepo) {
      // External repo: can't Apply (changes are in a different repo).
      // Show Done instead so the user can dismiss the thread.
      actions = DISMISS_ACTIONS;
    } else if (pendingChange?.requires_restart || ccInfo?.requiresRestart) {
      // Prefer the pending change's flag — it's the authoritative file-derived
      // value at proposal time. meta.ccRequiresRestart can lag (only set by
      // CodingAgentIdled) or be wrong (recovery fallback hardcodes false).
      requiresRestart = true;
    }
  }
  if (actions.length === 0) return null;

  return { type: 'actions', actions, threadId: focused, isDismissing: false, requiresRestart, pendingChange };
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

  return (
    <div class="thread-action-buttons">
      {state.pendingChange && (
        <button class="action-btn" onClick={() => viewChangeDiff(state.pendingChange!)}>Diff</button>
      )}
      {state.actions.map(action => renderActionButton(action, state.threadId, state.isDismissing, state.requiresRestart))}
    </div>
  );
}

function renderActionButton(action: Action, threadId: string, isDismissing: boolean, requiresRestart = false) {
  switch (action) {
    case 'done':
      return (
        <button class="action-btn" disabled={isDismissing}
          onClick={() => handleDismissThread(threadId)}>
          {isDismissing ? 'Done...' : 'Done'}
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
