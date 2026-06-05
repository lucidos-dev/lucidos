import type { ComponentChildren } from 'preact';
import { threadMap, focusedThreadId, applyingNowThreadIds, archivingThreadIds, discardingCCThreadIds, cancelingThreadIds, effectiveThreadStatus, isMidTurn } from '../../store/store';
import { resolveThreadActions, type TaggedAction } from '../../store/actions/threadActions';
import { viewThreadCcDiff } from '../../store/actions/repositories';

/** The close-set kinds the banner renders. Discard-draft (rendered by
 *  PromptInput's "Discard draft" button) and the Save/Unsave toggle (rendered
 *  by PromptInput's section buttons) are excluded — they come from the same
 *  selector but live in a different slot. */
const BANNER_CLOSE_KINDS: ReadonlySet<string> = new Set(['discard', 'apply', 'archive']);

type WaitingState =
  | { type: 'applying' }
  | { type: 'discarding' }
  | { type: 'canceling'; threadId: string; isCanceling: boolean }
  | { type: 'actions'; actions: TaggedAction[]; threadId: string; isArchiving: boolean; showDiff: boolean };

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
  // changes so the banner doesn't flash away mid-archive. (The selector returns
  // no Archive action once the optimistic section flips to 'archived', so this
  // dedicated flag is what keeps the spinner on screen.)
  if (archivingThreadIds.value.has(focused)) {
    return { type: 'actions', actions: [], threadId: focused, isArchiving: true, showDiff: false };
  }

  const status = effectiveThreadStatus(thread);

  // Mid-turn states get Cancel. Must come before the selector, which returns no
  // close actions for both and would otherwise drop us into the "no banner"
  // branch. Excludes 'waiting' (CC has changes — needs Apply/Discard, not Cancel).
  if (isMidTurn(status)) {
    // codingAgentApplying = MergeConflictDetected fired and the apply task is
    // driving the Claude Code session through a merge. The 'running' status reflects
    // that engine-pushed merge prompt, not a user turn. Cancel here would only
    // interrupt CC mid-merge — the apply task in the engine continues, sees
    // CC went idle, and emits ChangeApplied if the merge had already landed.
    // Show "Apply..." instead so the user can't trigger a no-op cancel.
    if (thread.meta.codingAgentApplying) return { type: 'applying' };
    return {
      type: 'canceling',
      threadId: focused,
      isCanceling: cancelingThreadIds.value.has(focused),
    };
  }

  // Close-set buttons come straight from the action-availability selector, so
  // their labels, confirms, handlers, the external-repo carve-out, and the
  // Apply restart/partial-work hints are all single-sourced (no enablement
  // drift vs the close cascade, which drives the same TaggedActions).
  const actions = resolveThreadActions(focused).filter((a) => BANNER_CLOSE_KINDS.has(a.kind));
  if (actions.length === 0) return null;

  // The Diff button is shown only when the CC branch actually has a diff on
  // disk (`codingAgentHasDiff` — single git-truth signal maintained by the
  // backend projection + recovery sweep, computed by the SAME algorithm the
  // Diff viewer renders). No diff → no button, so it can never drop the user
  // into an empty diff. This matches `getStandaloneCcDiffButton`, which also
  // hides when there's nothing to show.
  const showDiff = thread.meta.channel === 'claude_code' && thread.meta.codingAgentHasDiff;

  return { type: 'actions', actions, threadId: focused, isArchiving: false, showDiff };
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

  // Archive in flight: a dedicated disabled spinner (the selector no longer
  // returns an Archive action once the optimistic section flips).
  if (state.isArchiving) {
    return {
      liftable: null,
      primary: <button key="archive" class="action-btn" data-row-item disabled aria-label="Archive thread">Archive...</button>,
    };
  }

  // Diff always opens the thread-level branch diff. The historical
  // change-row Diff buttons (ChatExchange, ChangesView) call viewChangeDiff
  // for a specific Change; the WaitingBanner's affordance is "show me what
  // this thread's branch looks like right now" — backed by codingAgentHasDiff,
  // not by any one Change row.
  const actionButtons = state.actions.map((action) => renderActionButton(action));

  return {
    liftable: state.showDiff ? renderDiffButton(state.threadId) : null,
    primary: <>{actionButtons}</>,
  };
}

/** Shared Diff-button JSX. Rendered in two places: inside the banner via
 *  `getBannerSlots`, and as a standalone slot via `getStandaloneCcDiffButton`.
 *  Both call sites only render it when the branch has a diff to show, so the
 *  button is always clickable — no disabled form. Same key in both so Preact
 *  treats it as one node across banner ↔ standalone transitions. */
function renderDiffButton(threadId: string): ComponentChildren {
  return (
    <button
      key="diff"
      class="action-btn"
      data-row-item
      onClick={() => void viewThreadCcDiff(threadId)}
    >
      Diff
    </button>
  );
}

/** Diff button decoupled from waitingState: appears whenever the focused
 *  CC thread's branch has a diff, even mid-turn (when getWaitingState
 *  returns 'canceling' and the banner is suppressed). PromptInput uses this
 *  in the slots-fallback path so the user-facing rule "branch has a diff →
 *  Diff visible" holds regardless of CC's run-state. Same `codingAgentHasDiff`
 *  gate as the banner path, so both surfaces show/hide together. */
export function getStandaloneCcDiffButton(): ComponentChildren | null {
  const focused = focusedThreadId.value;
  if (!focused) return null;
  const thread = threadMap.value.get(focused);
  if (!thread) return null;
  if (thread.meta.channel !== 'claude_code') return null;
  if (!thread.meta.codingAgentHasDiff) return null;
  return renderDiffButton(focused);
}

/** Render one close-set TaggedAction. Class + aria derive from the action kind;
 *  label, tooltip, and the (confirm-wrapped) handler come from the selector. */
function renderActionButton(action: TaggedAction) {
  const cls =
    action.kind === 'discard'
      ? 'action-btn action-btn-danger'
      : action.kind === 'apply'
        ? 'action-btn action-btn-confirm'
        : 'action-btn';
  return (
    <button
      key={action.kind}
      class={cls}
      data-row-item
      aria-label={action.kind === 'archive' ? 'Archive thread' : undefined}
      data-tooltip={action.tooltip}
      onClick={() => void action.invoke()}
    >
      {action.label}
    </button>
  );
}
