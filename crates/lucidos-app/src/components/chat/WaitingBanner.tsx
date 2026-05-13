import type { ComponentChildren } from 'preact';
import { threadMap, focusedThreadId, applyingNowThreadIds, archivingThreadIds, discardingCCThreadIds, cancelingThreadIds, changes, showConfirm, effectiveThreadStatus, isMidTurn } from '../../store/store';
import { getCCWaitingInfo } from '../../store/thread-events';
import { resolveActions, type Action } from '../../generated/thread-lifecycle';
import { endClaudeCodeAndApply, handleDiscardCCChanges } from '../../store/actions/chat-claude-code';
import { handleArchiveThread } from '../../store/actions/threads';
import { viewChangeDiff, viewThreadCcDiff } from '../../store/actions/repositories';
import type { Change } from '../../api/client';

// Action buttons: Archive, Apply, Discard — never "Requesting"
const ARCHIVE_ACTIONS: Action[] = ['archive'];

export const DIFF_DISABLED_TOOLTIP = 'No changes on this branch';

type CcDiff = 'hidden' | 'disabled' | 'enabled';

type WaitingState =
  | { type: 'applying' }
  | { type: 'discarding' }
  | { type: 'canceling'; threadId: string; isCanceling: boolean }
  | { type: 'actions'; actions: Action[]; threadId: string; isArchiving: boolean; requiresRestart: boolean; incomplete: boolean; pendingChange: Change | null; ccDiff: CcDiff };

/** Banner state passed to `getBannerSlots`. The 'canceling' variant is owned
 *  by PromptInput's morphable Send→Cancel button (so the swap can animate the
 *  same DOM node) and must never be passed here — narrow it out at the call
 *  site. */
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
    return { type: 'actions', actions: ARCHIVE_ACTIONS, threadId: focused, isArchiving: true, requiresRestart: false, incomplete: false, pendingChange: null, ccDiff: 'hidden' };
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

  // Show Diff disabled (rather than hiding) when no signal indicates branch
  // work, so CC threads always advertise the affordance without dropping
  // the user into an empty diff.
  let ccDiff: CcDiff;
  if (threadType !== 'claude_code') {
    ccDiff = 'hidden';
  } else if (!!pendingChange || (ccInfo?.hasChanges ?? false) || thread.meta.ccIsExternalRepo) {
    ccDiff = 'enabled';
  } else {
    ccDiff = 'disabled';
  }

  // Surface partial-work warnings for changes proposed from a CC turn that
  // ended in `ResponseFailed` (mid-stream API drop, etc.). The Apply button
  // confirms before landing so the user can't accidentally merge half a turn.
  const incomplete = pendingChange?.incomplete ?? false;

  return { type: 'actions', actions, threadId: focused, isArchiving: false, requiresRestart, incomplete, pendingChange, ccDiff };
}

interface BannerSlots {
  /** The single secondary item the parent may move onto a row above when the
   *  natural single-row layout would overflow — Diff for the actions state.
   *  `null` when there is nothing worth lifting (the busy "Apply..." /
   *  "Discard..." spinners and Diff-less actions all fit naturally). */
  liftable: ComponentChildren | null;
  /** Action buttons that always render on the bottom row, anchored to the
   *  right. PromptInput renders sectionButtons (Save / ✓ Saved) just before
   *  these — never inside the lift sub-row, so the bottom row stays
   *  [icons][Save][Discard][Apply] when there is room for it. */
  primary: ComponentChildren;
}

/** Splits the banner's buttons into liftable + primary slots so the caller
 *  (PromptInput) can decide whether to render them as one row or stack the
 *  liftable slot above the row that holds the icons. PromptInput owns where
 *  Save / ✓ Saved goes (always in the bottom row, before the action buttons),
 *  so getBannerSlots only worries about the action-side layout. When there's
 *  room, [Save][Diff][Discard][Apply] sit together; when there isn't, only
 *  Diff hops to a row above and [Save][Discard][Apply] stay on the bottom. */
export function getBannerSlots(state: BannerState): BannerSlots {
  if (state.type === 'applying') {
    return {
      liftable: null,
      primary: <button key="applying" class="action-btn action-btn-confirm" data-row-item disabled>Apply...</button>,
    };
  }

  if (state.type === 'discarding') {
    return {
      liftable: null,
      primary: <button key="discarding" class="action-btn action-btn-danger" data-row-item disabled>Discard...</button>,
    };
  }

  const change = state.pendingChange;
  const diffOnClick = change
    ? () => viewChangeDiff(change)
    : () => viewThreadCcDiff(state.threadId);
  const actionButtons = state.actions.map(action =>
    renderActionButton(action, state.threadId, state.isArchiving, state.requiresRestart, state.incomplete),
  );

  const enabled = state.ccDiff === 'enabled';
  return {
    liftable: state.ccDiff === 'hidden'
      ? null
      : (
        <button
          key="diff"
          class="action-btn"
          data-row-item
          disabled={!enabled}
          data-tooltip={enabled ? undefined : DIFF_DISABLED_TOOLTIP}
          onClick={diffOnClick}
        >
          Diff
        </button>
      ),
    primary: <>{actionButtons}</>,
  };
}

function renderActionButton(action: Action, threadId: string, isArchiving: boolean, requiresRestart = false, incomplete = false) {
  switch (action) {
    case 'archive':
      return (
        <button key="archive" class="action-btn" data-row-item disabled={isArchiving}
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
        <button key="apply" class="action-btn action-btn-confirm" data-row-item
          data-tooltip={tooltip}
          onClick={onClick}>
          {requiresRestart ? 'Apply & Restart' : 'Apply'}
        </button>
      );
    }
    case 'discard':
      return (
        <button key="discard" class="action-btn action-btn-danger" data-row-item
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
