import type { ComponentChildren } from 'preact';
import { threadMap, focusedThreadId, applyingNowThreadIds, applyingChangeThreadIds, archivingThreadIds, discardingCCThreadIds, cancelingThreadIds, effectiveThreadStatus, isMidTurn } from '../../store/store';
import { resolveThreadActions, type TaggedAction } from '../../store/actions/threadActions';
import { viewThreadCcDiff } from '../../store/actions/repositories';
import { SplitButton, type SplitButtonMenuItem } from '../shared/SplitButton';

/** The close-set kinds the banner renders. Discard-draft (a compose-draft
 *  action resolved by the close-cascade shortcut, not the banner) and the
 *  Pin/Unpin toggle (rendered by PromptInput's section buttons) are excluded —
 *  they come from the same selector but live in a different slot. */
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

  // Apply Now in progress — show disabled "Apply...". This flow stops the
  // session and commits the change; there's no interruptible turn, so it stays
  // non-cancelable (unlike the harden/merge sessions handled by the mid-turn
  // branch below). The Archive button must never render while apply is active;
  // the actions (handleArchiveThread / endClaudeCodeAndApply) enforce mutual
  // exclusivity, so applying, dismissing, and discarding can't coexist.
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
  // close actions and would otherwise drop us into the "no banner" branch.
  // Excludes 'waiting' (CC has changes — needs Apply/Discard, not Cancel).
  //
  // This now also covers an apply that woke a live Claude Code session — a
  // `/harden` run (codingAgentApplying false, status running) or a merge-conflict
  // resolution (codingAgentApplying true, status running). Both run as a real CC
  // turn, so the user can interrupt them. Cancel is best-effort for a merge: if
  // the merge already landed before the interrupt processes, the engine still
  // emits ChangeApplied; otherwise the change returns to pending. (Previously a
  // merge showed a disabled "Apply..." here, leaving no way to stop a long-
  // running merge — that's the regression this branch fixes.)
  if (isMidTurn(status)) {
    return {
      type: 'canceling',
      threadId: focused,
      isCanceling: cancelingThreadIds.value.has(focused),
    };
  }

  // Applying but NOT mid-turn — a queued Apply All member waiting its turn: its
  // change is marked applying (reverse-mapped via applyingChangeThreadIds) but no
  // live session is running yet, so there's nothing to interrupt. Show disabled
  // "Apply...". The actively-hardening/merging member hits the mid-turn Cancel
  // branch above instead.
  if (applyingChangeThreadIds.value.has(focused)) return { type: 'applying' };

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
   *  natural single-row layout would overflow — the standalone Diff button
   *  whenever the branch has a diff. `null` when there is nothing worth lifting
   *  (the busy "Apply..." / "Discard..." spinners and Diff-less actions all fit
   *  naturally). */
  liftable: ComponentChildren | null;
  /** Action buttons that always render on the bottom row, anchored to the
   *  right. PromptInput renders sectionButtons (Pin / ✓ Pinned) just before
   *  these — never inside the lift sub-row, so the bottom row stays
   *  [icons][Pin][Discard][Apply] when there is room for it. */
  primary: ComponentChildren;
}

/** Splits the banner's buttons into liftable + primary slots so the caller
 *  (PromptInput) can decide whether to render them as one row or stack the
 *  liftable slot above the row that holds the icons. PromptInput owns where
 *  Pin / ✓ Pinned goes (always in the bottom row, before the action buttons),
 *  so getBannerSlots only worries about the action-side layout. Diff is ALWAYS
 *  its own standalone button in the liftable slot — never folded into the
 *  Apply/Discard cluster — so when there's room [Pin][Diff][Discard][Apply]
 *  sit together, and when there isn't only Diff hops to a row above while
 *  [Pin][Discard][Apply] stay on the bottom. */
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

  // Diff is ALWAYS its own standalone button in the liftable slot — never folded
  // into the Apply/Discard cluster. It shows whenever the branch has a diff on
  // disk (state.showDiff), independent of whether the close set has an Apply.
  const liftable = state.showDiff ? renderDiffButton(state.threadId) : null;

  // When the close set has a primary Apply action, collapse the remaining
  // close-set buttons into a split button — a one-tap "Apply (& Restart)" face
  // plus a caret menu holding the other action(s) (Discard, Archive). One
  // compact control instead of two or three buttons that overflowed the prompt
  // row. Diff sits outside it, in the liftable slot above. The separate-button
  // path below runs for the no-Apply states (e.g. an idle CC thread with a diff
  // but no pending change → Archive + standalone Diff).
  const applyAction = state.actions.find((a) => a.kind === 'apply');
  if (applyAction) {
    const menuActions = state.actions.filter((a) => a !== applyAction);
    return {
      liftable,
      primary: (
        <ChangeActionSplitButton
          primary={applyAction}
          menuActions={menuActions}
        />
      ),
    };
  }

  // Diff always opens the thread-level branch diff. The historical
  // change-row Diff buttons (ChatExchange, ChangesView) call viewChangeDiff
  // for a specific Change; the WaitingBanner's affordance is "show me what
  // this thread's branch looks like right now" — backed by codingAgentHasDiff,
  // not by any one Change row.
  const actionButtons = state.actions.map((action) => renderActionButton(action));

  return {
    liftable,
    primary: <>{actionButtons}</>,
  };
}

/** Change-action split button: a one-tap primary face (Apply / Apply & Restart)
 *  plus a caret menu holding the remaining close-set actions (Discard, Archive).
 *  Diff is NOT in here — it lives permanently outside this cluster as its own
 *  standalone button (getBannerSlots' liftable slot). Built on the generic
 *  `SplitButton` (the same control the prompt's multi-select answer Submit
 *  uses), so the caret / Overlay-dismiss / inert-primary contract lives in one
 *  place. Labels, tooltips, and handlers all come from the same TaggedActions
 *  the desktop buttons use, so there's no enablement drift. */
function ChangeActionSplitButton({
  primary,
  menuActions,
}: {
  primary: TaggedAction;
  menuActions: TaggedAction[];
}) {
  const menuItems: SplitButtonMenuItem[] = menuActions.map((action) => ({
    key: action.kind,
    label: action.label,
    className: action.kind === 'discard' ? 'action-btn action-btn-danger' : 'action-btn',
    tooltip: action.tooltip,
    onClick: () => void action.invoke(),
  }));
  return (
    <SplitButton
      primaryLabel={primary.label}
      primaryClassName="action-btn action-btn-confirm"
      primaryTooltip={primary.tooltip}
      onPrimary={() => void primary.invoke()}
      caretClassName="action-btn action-btn-confirm"
      caretAriaLabel="More change actions"
      menuItems={menuItems}
    />
  );
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
