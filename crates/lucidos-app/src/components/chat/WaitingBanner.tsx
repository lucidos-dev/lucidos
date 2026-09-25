import type { ComponentChildren } from 'preact';
import { threadMap, focusedThreadId, applyingNowThreadIds, applyingChangeThreadIds, archivingThreadIds, discardingCCThreadIds, cancelingThreadIds, effectiveThreadStatus, isMidTurn, standingApplyThreadIds, armingStandingApplyThreadIds } from '../../store/store';
import { resolveThreadActions, type TaggedAction } from '../../store/actions/threadActions';
import type { ThreadState } from '../../store/thread-events';
import { viewThreadCcDiff } from '../../store/actions/repositories';
import { SplitButton, type SplitButtonMenuItem } from '../shared/SplitButton';
import { ArchiveIcon, CheckIcon, DiffIcon, StandingApplyIcon, TrashIcon } from '../shared/icons';
import type { HeaderActionSpec } from '../layout/headerActions';
import type { OverflowMenuContext } from '../shared/OverflowMenu';
import { useTouchActivated } from '../../hooks/useTouchActivated';
import { blurPromptInputIfFocused } from './promptFocus';

/** The close-set kinds the banner renders. Two other kinds come from the same
 *  selector and live elsewhere, so both are excluded. Discard-draft is a
 *  compose-draft action the close-cascade shortcut resolves. Pin/Unpin is
 *  `PinThreadButton`, in the thread header. */
const BANNER_CLOSE_KINDS: ReadonlySet<string> = new Set(['discard', 'apply', 'archive']);

type WaitingState =
  | { type: 'applying' }
  | { type: 'discarding' }
  | { type: 'canceling'; threadId: string; isCanceling: boolean }
  | { type: 'actions'; actions: TaggedAction[]; threadId: string; isArchiving: boolean; showDiff: boolean };

/** Banner state passed to `getBannerActions`. The 'canceling' variant is owned
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
  // into an empty diff. `getStandaloneActions` reads the same gate.
  const showDiff = hasCcDiff(thread);

  return { type: 'actions', actions, threadId: focused, isArchiving: false, showDiff };
}

/** ONE close-set action, as a ⋯ menu row. The composite split button folds into
 *  several of these, so the caret's actions survive the fold with the face. */
function actionMenuRow(action: TaggedAction, ctx: OverflowMenuContext) {
  return (
    <button
      key={action.kind}
      type="button"
      class="thread-overflow-item"
      role="menuitem"
      onClick={ctx.run(() => void action.invoke())}
    >
      {ACTION_ICON[action.kind]?.() ?? null}
      {action.label}
    </button>
  );
}

/** A glyph per close-set kind, for the menu row. The row buttons carry words,
 *  so this exists only for the folded rendering. */
const ACTION_ICON: Record<string, () => ComponentChildren> = {
  apply: () => <CheckIcon />,
  discard: () => <TrashIcon />,
  archive: () => <ArchiveIcon />,
};

/** The banner's members, in FOLD ORDER, as specs the composer row can fold.
 *
 *  Diff folds before the change actions: looking is cheaper to postpone than
 *  doing. A busy state is one member, the spinner, and it folds like any other
 *  rather than being a reason to overflow the box. */
export function getBannerActions(state: BannerState): HeaderActionSpec[] {
  if (state.type === 'applying') return [busyAction('applying', 'action-btn-confirm', 'Apply...')];
  if (state.type === 'discarding') return [busyAction('discarding', 'action-btn-danger', 'Discard...')];
  // Archive in flight: a dedicated disabled spinner (the selector no longer
  // returns an Archive action once the optimistic section flips).
  if (state.isArchiving) return [busyAction('archiving', '', 'Archive...')];

  const members: HeaderActionSpec[] = [];
  // Diff shows whenever the branch has a diff on disk, independent of whether
  // the close set has an Apply. It always opens the thread-level branch diff.
  // The historical change-row Diff buttons (ChatExchange, ChangesView) call
  // viewChangeDiff for one Change; this asks what the branch looks like now.
  if (state.showDiff) members.push(diffAction(state.threadId));

  // When the close set has a primary Apply, the remaining close-set buttons
  // collapse into a split button: a one-tap face plus a caret menu holding the
  // others. One compact control instead of two or three. Folded, it is several
  // menu rows, because the caret's actions would otherwise go with it.
  const applyAction = state.actions.find((a) => a.kind === 'apply');
  if (applyAction) {
    const menuActions = state.actions.filter((a) => a !== applyAction);
    members.push({
      key: 'change-actions',
      label: applyAction.label,
      tooltip: applyAction.tooltip,
      icon: () => <CheckIcon />,
      render: (attrs) => (
        <ChangeActionSplitButton
          primary={applyAction}
          menuActions={menuActions}
          attrs={attrs}
        />
      ),
      menuRows: (ctx) => [applyAction, ...menuActions].map((a) => actionMenuRow(a, ctx)),
    });
    return members;
  }

  // The no-Apply states (an idle coding-agent thread with a diff but no pending
  // change, so Archive plus a standalone Diff). Each button is its own member,
  // so the row can fold one without the other.
  for (const action of state.actions) members.push(closeSetAction(action));
  return members;
}

/** The members the row carries when the banner is SUPPRESSED because the thread
 *  has not settled (working, or watching an event): the same Diff, and the
 *  standing apply.
 *
 *  Diff is decoupled from `waitingState` so the user-facing rule "branch has a
 *  diff, Diff visible" holds whatever the coding agent's run-state. The
 *  standing apply is here because that row used to lift Diff and nothing else.
 *  A working thread then offered no way to arm an apply at all. */
export function getStandaloneActions(): HeaderActionSpec[] {
  const focused = focusedThreadId.value;
  if (!focused) return [];
  const members: HeaderActionSpec[] = [];
  const thread = threadMap.value.get(focused);
  if (thread && hasCcDiff(thread)) members.push(diffAction(focused));
  const standing = resolveThreadActions(focused).find((a) => a.kind === 'apply_when_settled');
  if (standing) {
    members.push({
      key: 'standing-apply',
      dataRole: 'standing-apply',
      label: standing.label,
      tooltip: standing.tooltip,
      icon: () => <StandingApplyIcon armed={standingApplyThreadIds.value.has(focused)} />,
      render: (attrs) => (
        <StandingApplyButton threadId={focused} action={standing} attrs={attrs} />
      ),
      onClick: () => void standing.invoke(),
    });
  }
  return members;
}

/** The one Diff gate, shared by the banner and the standalone row so both
 *  surfaces show and hide Diff together. */
function hasCcDiff(thread: ThreadState): boolean {
  return thread.meta.channel === 'claude_code' && thread.meta.codingAgentHasDiff;
}

function diffAction(threadId: string): HeaderActionSpec {
  return {
    key: 'thread-diff',
    dataRole: 'thread-diff',
    label: 'Diff',
    tooltip: 'Show what this thread changed',
    icon: () => <DiffIcon />,
    render: (attrs) => <DiffButton threadId={threadId} attrs={attrs} />,
    onClick: () => {
      blurPromptInputIfFocused();
      void viewThreadCcDiff(threadId);
    },
  };
}

/** A request in flight. Disabled on both sides: `disabledTooltip` is what makes
 *  the folded row `aria-disabled` rather than a live action. */
function busyAction(key: string, variant: string, label: string): HeaderActionSpec {
  return {
    key,
    label,
    disabledTooltip: label,
    icon: () => null,
    render: (attrs) => (
      <button {...attrs} class={`action-btn ${variant}`.trim()} disabled aria-label={label}>
        {label}
      </button>
    ),
  };
}

/** Render one close-set TaggedAction as a member. Class and aria derive from
 *  the kind; label, tooltip and the (confirm-wrapped) handler come from the
 *  selector. */
function closeSetAction(action: TaggedAction): HeaderActionSpec {
  const cls =
    action.kind === 'discard'
      ? 'action-btn action-btn-danger'
      : action.kind === 'apply'
        ? 'action-btn action-btn-confirm'
        : 'action-btn';
  const label = action.kind === 'archive' ? 'Archive thread' : action.label;
  return {
    key: action.kind,
    label,
    tooltip: action.tooltip,
    icon: () => ACTION_ICON[action.kind]?.() ?? null,
    render: (attrs) => (
      <button
        {...attrs}
        class={cls}
        aria-label={action.kind === 'archive' ? label : undefined}
        data-tooltip={action.tooltip}
        onClick={() => void action.invoke()}
      >
        {action.label}
      </button>
    ),
    onClick: () => void action.invoke(),
  };
}

/** Change-action split button: a one-tap primary face (Apply / Apply*)
 *  plus a caret menu holding the remaining close-set actions (Discard, Archive).
 *  Diff is NOT in here — it lives permanently outside this cluster as its own
 *  member of `getBannerActions`. Built on the generic
 *  `SplitButton` (the same control the prompt's multi-select answer Submit
 *  uses), so the caret / Overlay-dismiss / inert-primary contract lives in one
 *  place. Labels, tooltips, and handlers all come from the same TaggedActions
 *  the desktop buttons use, so there's no enablement drift. */
function ChangeActionSplitButton({
  primary,
  menuActions,
  attrs,
}: {
  primary: TaggedAction;
  menuActions: TaggedAction[];
  attrs?: Record<string, string>;
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
      attrs={attrs}
    />
  );
}

/** The Diff button. Rendered in two places: inside the banner via
 *  `getBannerActions`, and on the standalone row via `getStandaloneActions`.
 *  Both call sites only render it when the branch has a diff to show, so the
 *  button is always clickable, with no disabled form. Same key in both so Preact
 *  treats it as one node across banner and standalone transitions.
 *
 *  An ICON, wearing the shape the standing apply beside it already wears. The
 *  blue pill it replaces was the widest control on a phone's prompt row. The
 *  word survives as the `aria-label`, and `data-tooltip-longpress` puts the
 *  tooltip within reach of a finger (`hooks/useTooltip.ts`).
 *
 *  TOUCH ACTIVATED, like the composer's Send and its answer Submit beside it.
 *  The user reaches this row with the mobile keyboard up, and there WebKit
 *  drops the synthetic click. It was reported dead in exactly that state. Diff
 *  is non-destructive and idempotent, so it takes the touch path.
 *
 *  A component rather than a function returning JSX, because it holds a hook and
 *  `getBannerActions` is called conditionally from `PromptInput`'s render.
 *
 *  It drops the keyboard itself, because nothing else will now: the shared
 *  `installActionBtnBlurListener` fires for an `.action-btn`, which this no
 *  longer is, and a touch-activated face suppresses the click it listens on. */
export function DiffButton({ threadId, attrs }: { threadId: string; attrs?: Record<string, string> }) {
  const activate = useTouchActivated(() => {
    blurPromptInputIfFocused();
    void viewThreadCcDiff(threadId);
  });
  return (
    <button
      {...attrs}
      class="icon-btn header-icon"
      data-role="thread-diff"
      aria-label="Diff"
      data-tooltip="Show what this thread changed"
      data-tooltip-longpress=""
      onTouchEnd={activate.onTouchEnd}
      onClick={activate.onClick}
    >
      <DiffIcon />
    </button>
  );
}

/** The change action a still-working thread offers: arm a *standing apply*, or
 *  cancel the one it carries (ADR 0168 clause 5).
 *
 *  An ICON toggle, wearing the shape the follow toggle beside it already wears
 *  (`PromptRowControls`). As a green pill it took over half a phone's prompt
 *  row. No label short enough to fix that still said what it does.
 *
 *  The word survives twice over, because the label was the only explanation a
 *  phone had. The `aria-label` IS the action's own label, the string the
 *  Changes panel shows as visible text. And `data-tooltip-longpress` is what
 *  puts the tooltip within reach of a finger: the host shell reveals on a long
 *  press only for elements carrying it, so a `data-tooltip` alone would be
 *  desktop-only (`hooks/useTooltip.ts`).
 *
 *  Never disabled. A tap while the request is in flight is dropped by the
 *  handler instead, because `.icon-btn:disabled` sets `pointer-events: none`
 *  and takes that tooltip with it.
 *
 *  It does NOT blur the composer, unlike the `.action-btn` it used to be. This
 *  arms a mode rather than closing the thread. So a reader typing a follow-up
 *  keeps their keyboard, as the toggles beside it already leave it alone.
 *
 *  A component rather than a function returning JSX, because it reads signals
 *  and `getStandaloneActions` is called from `PromptInput`'s render. */
export function StandingApplyButton({
  threadId,
  action,
  attrs,
}: {
  threadId: string;
  action: TaggedAction;
  attrs?: Record<string, string>;
}) {
  const busy = armingStandingApplyThreadIds.value.has(threadId);
  const armed = standingApplyThreadIds.value.has(threadId);
  return (
    <button
      {...attrs}
      class={`icon-btn header-icon${armed ? ' active' : ''}`}
      data-role="standing-apply"
      aria-pressed={armed}
      aria-label={action.label}
      data-tooltip={action.tooltip}
      data-tooltip-longpress=""
      onClick={() => {
        if (busy) return;
        void action.invoke();
      }}
    >
      <StandingApplyIcon armed={armed} />
    </button>
  );
}

/** The change action for the focused thread's own prompt row, when the banner
 *  is suppressed because the thread has not settled (working, or watching an
 *  event).
 *
 *  That row used to lift the Diff button and nothing else, so a working
 *  coding-agent thread offered no way to arm an apply at all. Availability
 *  comes from the same lifecycle selector the banner reads, so the two cannot
 *  drift on when the action exists. */
export function getStandingApplyControl(): ComponentChildren | null {
  const focused = focusedThreadId.value;
  if (!focused) return null;
  const action = resolveThreadActions(focused).find((a) => a.kind === 'apply_when_settled');
  if (!action) return null;
  return <StandingApplyButton key="standing-apply" threadId={focused} action={action} />;
}
