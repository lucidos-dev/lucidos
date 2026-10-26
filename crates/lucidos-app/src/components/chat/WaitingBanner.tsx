import type { ComponentChildren } from 'preact';
import { threadMap, focusedThreadId, getThreadDisplaySection, applyingNowThreadIds, applyingChangeThreadIds, discardingCCThreadIds, cancelingThreadIds, effectiveThreadStatus, isMidTurn, threadUnsettled, standingApplyThreadIds, armingStandingApplyThreadIds, setAsideChangeForThread, pendingChangeForThread, landingLane, lanePlace, type LanePlace } from '../../store/store';
import { LANE_PLACE_LABEL, LANE_THREAD_TIP } from '../../store/actions/landingLane';
import { archiveRequestInFlight } from '../../store/rowIntents';
import { bringBackSingleChange, discardSingleChange } from '../../store/actions/chat-changes';
import { resolveThreadActions, type TaggedAction } from '../../store/actions/threadActions';
import { focusThreadOrBootstrap, handleArchiveThread, showBlockedToast } from '../../store/actions/threads';
import { BLOCKER_REASON, isDescendantBlocker, type BlockedRefusal } from '../../store/actions/blockerCopy';
import { exitItems, threadBlocker } from '../../store/actions/threadBlockers';
import type { ThreadState } from '../../store/thread-events';
import { viewThreadCcDiff } from '../../store/actions/repositories';
import { ToggleIconButton } from '../shared/ToggleIconButton';
import { ArchiveIcon, CheckIcon, DiffIcon, InFlightIcon, MoreIcon, MoveToTopIcon, SetAsideIcon, StandingApplyIcon, TrashIcon } from '../shared/icons';
import { renderMenuAction, type HeaderActionSpec } from '../layout/headerActions';
import { OverflowMenu } from '../shared/OverflowMenu';
import { useTouchActivated } from '../../hooks/useTouchActivated';
import { blurPromptInputIfFocused } from './promptFocus';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { getDraft } from '../../store/composeDrafts';
import { hasInFlightUploads } from '../../store/pendingUploads';
import { composeHasContent } from './prompt-input-helpers';
import type { SlotAction, SlotOccupant, SlotTone } from './ActionSlot';

const DIFF_TOOLTIP = 'Show what this thread changed';

const DIFF_KEY = 'thread-diff';
const STANDING_APPLY_KEY = 'standing-apply';

/** Diff and the standing apply, which the composer row may draw as ONE control
 *  (`DiffApplyMenu`) before its follow toggle folds. `keys` are adjacent in the
 *  standalone row's fold order, and `foldKey` names the merged control's box. */
export const DIFF_APPLY_MERGE = {
  keys: [DIFF_KEY, STANDING_APPLY_KEY],
  foldKey: 'diff-and-standing-apply',
} as const;

/** The close-set kinds the banner renders. Two other kinds come from the same
 *  selector and live elsewhere, so both are excluded. Discard-draft is a
 *  compose-draft action the close-cascade shortcut resolves. Pin/Unpin is
 *  `PinThreadButton`, in the thread header. */
const BANNER_CLOSE_KINDS: ReadonlySet<string> = new Set(['discard', 'apply', 'set_aside', 'archive']);

const BRING_BACK_TOOLTIP =
  "This thread's change is set aside. Bring it back to review and apply it.";

const DISCARD_SET_ASIDE_TOOLTIP = "Throw away this thread's set-aside change.";

type WaitingState =
  | { type: 'applying' }
  /** The thread's change is in the landing lane, which decides when it lands. */
  | { type: 'landing'; place: LanePlace; laneThreadId: string | null }
  | { type: 'discarding' }
  | { type: 'canceling'; threadId: string; isCanceling: boolean }
  | {
      type: 'actions';
      actions: TaggedAction[];
      threadId: string;
      isArchiving: boolean;
      showDiff: boolean;
      /** The thread's set-aside change, which the banner offers to bring back,
       *  and to discard unless the thread is unsettled. */
      setAside?: { changeId: string; discardable: boolean };
      /** A sub-thread holds Archive back. The thread's own blockers already
       *  draw the control that resolves them: Cancel, Apply or Discard. */
      blockedArchive?: BlockedRefusal;
    };

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

  // Apply Now in progress — show disabled "Apply...". The apply's own
  // hardening and merge turns are the exception: they fall through to the
  // mid-turn branch below, which keeps their Stop. The Archive button must
  // never render while apply is active; the actions (handleArchiveThread /
  // endClaudeCodeAndApply) enforce mutual exclusivity, so applying,
  // dismissing, and discarding can't coexist.
  if (applyingNowThreadIds.value.has(focused) && !isMidTurn(effectiveThreadStatus(thread))) {
    return { type: 'applying' };
  }

  // Discarding in progress — show "Discard..." and block all other actions.
  if (discardingCCThreadIds.value.has(focused)) return { type: 'discarding' };

  // Archive in progress — keep showing "Archive..." regardless of SSE state
  // changes so the banner doesn't flash away mid-archive. (The selector returns
  // no Archive action once the optimistic section flips to 'archived', so the
  // in-flight row intent is what keeps the spinner on screen.)
  if (archiveRequestInFlight(focused)) {
    return { type: 'actions', actions: [], threadId: focused, isArchiving: true, showDiff: false };
  }

  const status = effectiveThreadStatus(thread);

  // Mid-turn states get Cancel. Must come before the selector, which returns no
  // close actions and would otherwise drop us into the "no banner" branch.
  // Excludes 'waiting' (CC has changes — needs Apply/Discard, not Cancel).
  //
  // This now also covers an apply that woke a live Claude Code session — a
  // `/harden` run or a merge-conflict resolution, both with status running.
  // Both run as a real CC
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

  // The change waits in the landing lane, or lands in its open batch. The lane
  // decides when it lands, so the row says where it stands in place of Apply.
  const lane = landingLane.value;
  const place = lanePlace(lane, pendingChangeForThread(focused)?.id);
  if (place) return { type: 'landing', place, laneThreadId: lane.open_batch?.lane_thread_id ?? null };

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
  // A set-aside change keeps a way back on its own thread, archived or not.
  const setAsideChangeId = setAsideChangeForThread(focused)?.id;
  const setAside = setAsideChangeId
    ? { changeId: setAsideChangeId, discardable: !threadUnsettled(thread) }
    : undefined;
  // Placement is where the drawer shows the thread, not where it is stored
  // (ADR 0378). One in the Archive section offers nothing here: the thread's
  // ⋯ menu moves it back to Current.
  const archived = getThreadDisplaySection(thread) === 'archive';
  const blockedArchive = actions.length === 0 && !archived ? subThreadBlocker(focused) : undefined;
  // The thread menu's own answer, when the selector offers no Archive: a thread
  // stored archived but kept in Current, or one whose blocking sub-thread is
  // not loaded. Pressing it lets the engine decide, and recount if it drifted.
  if (actions.length === 0 && !archived && !blockedArchive && exitItems(focused).archive === 'enabled') {
    actions.push(archiveExitAction(focused));
  }
  if (actions.length === 0 && !setAside && !blockedArchive) return null;

  // The Diff button is shown only when the branch holds work (`hasCcDiff`), so
  // it can never drop the user into an empty diff. `getStandaloneActions`
  // reads the same gate.
  const showDiff = hasCcDiff(thread);

  return {
    type: 'actions',
    actions,
    threadId: focused,
    isArchiving: false,
    showDiff,
    setAside,
    blockedArchive,
  };
}

/** Archive the menu offers where the selector, reading stored facts, does not. */
function archiveExitAction(threadId: string): TaggedAction {
  return { kind: 'archive', category: 'close', label: 'Archive', invoke: async () => { await handleArchiveThread(threadId); } };
}

/** What holds Archive back when a sub-thread does, so the composer can say so
 *  rather than drawing nothing (ADR 0378). */
function subThreadBlocker(threadId: string): BlockedRefusal | undefined {
  const { blocker, subThreads } = threadBlocker(threadId);
  return blocker !== 'none' && isDescendantBlocker(blocker)
    ? { blocker, subThreadId: subThreads[0]?.thread.meta.id ?? null }
    : undefined;
}

/** A glyph per close-set kind: the icon its face pins at the left. */
const ACTION_ICON: Record<string, () => ComponentChildren> = {
  apply: () => <CheckIcon />,
  discard: () => <TrashIcon />,
  set_aside: () => <SetAsideIcon />,
  archive: () => <ArchiveIcon />,
};

/** The banner's members. Diff stands left of the model picker and folds like
 *  any member. Everything else is ONE control in the action slot, where Send
 *  stands, and it never folds. */
export interface BannerActions {
  situational: HeaderActionSpec[];
  slot: SlotOccupant;
}

export function getBannerActions(state: BannerState): BannerActions {
  if (state.type === 'applying') return alone(busyAction('applying', 'Apply', 'confirm', 'Applying'));
  if (state.type === 'landing') return alone(laneAction(state));
  if (state.type === 'discarding') return alone(busyAction('discarding', 'Discard', 'danger', 'Discarding'));
  // Archive in flight: a dedicated busy face (the selector no longer returns an
  // Archive action once the optimistic section flips).
  if (state.isArchiving) return alone(busyAction('archiving', 'Archive', 'neutral', 'Archiving'));

  // Diff shows whenever the branch has a diff on disk, independent of whether
  // the close set has an Apply. It always opens the thread-level branch diff.
  // The historical change-row Diff buttons (ChatExchange, ChangesView) call
  // viewChangeDiff for one Change; this asks what the branch looks like now.
  return {
    situational: state.showDiff ? [diffAction(state.threadId)] : [],
    slot: closeSlot(state),
  };
}

function alone(face: SlotAction): BannerActions {
  return { situational: [], slot: { face, menu: [], menuLabel: '' } };
}

/** Which action wears the face, first to last. Discard goes last, so the
 *  destructive choice sits furthest from the face. */
const FACE_ORDER = ['apply', 'archive', 'archive-blocked', 'set_aside', 'bring-back', 'discard', 'discard-set-aside'];

/** The close set, the set-aside pair and a held-back Archive, merged into one
 *  control. Its face is the first of them in `FACE_ORDER`, and the rest go in
 *  its menu. */
function closeSlot(state: Extract<BannerState, { type: 'actions' }>): SlotOccupant {
  const [face, ...menu] = [
    ...state.actions.map(closeSetAction),
    ...(state.setAside ? setAsideActions(state.setAside) : []),
    ...(state.blockedArchive ? [blockedArchiveAction(state.blockedArchive)] : []),
  ].sort((a, b) => FACE_ORDER.indexOf(a.key) - FACE_ORDER.indexOf(b.key));
  const menuLabel = face.key === 'apply' ? 'More change actions'
    : face.key === 'bring-back' ? 'More set-aside actions'
    : 'More close actions';
  return { face, menu, menuLabel };
}

/** One close-set TaggedAction. Label, tooltip and the (confirm-wrapped)
 *  handler come from the selector; tone and icon derive from the kind. */
function closeSetAction(action: TaggedAction): SlotAction {
  const tone: SlotTone = action.kind === 'discard' ? 'danger' : action.kind === 'apply' ? 'confirm' : 'neutral';
  const tooltip = action.tooltip ?? action.label;
  return {
    key: action.kind,
    label: action.label,
    ariaLabel: action.kind === 'archive' ? 'Archive thread' : undefined,
    tooltip: action.kind === 'apply' ? tooltipWithShortcut(tooltip, 'applyChange') : tooltip,
    tone,
    icon: () => ACTION_ICON[action.kind]?.() ?? null,
    invoke: () => void action.invoke(),
  };
}

/** Bring a set-aside change back to pending, from its own thread, with Discard
 *  beside it, as the Changes panel's set-aside row offers. Discard is withheld
 *  while the thread is unsettled, since the engine refuses it then. */
function setAsideActions({ changeId, discardable }: { changeId: string; discardable: boolean }): SlotAction[] {
  const bringBack: SlotAction = {
    key: 'bring-back',
    label: 'Bring back',
    tooltip: BRING_BACK_TOOLTIP,
    tone: 'neutral',
    icon: () => <MoveToTopIcon />,
    invoke: () => void bringBackSingleChange(changeId),
  };
  const discard: SlotAction = {
    key: 'discard-set-aside',
    label: 'Discard',
    ariaLabel: 'Discard the set-aside change',
    tooltip: DISCARD_SET_ASIDE_TOOLTIP,
    tone: 'danger',
    icon: () => <TrashIcon />,
    invoke: () => void discardSingleChange(changeId),
  };
  return discardable ? [bringBack, discard] : [bringBack];
}

/** Archive, held back by a sub-thread. It stays tappable: a tap says why, and
 *  the toast opens the sub-thread to resolve. */
function blockedArchiveAction(refusal: BlockedRefusal): SlotAction {
  const reason = BLOCKER_REASON[refusal.blocker];
  return {
    key: 'archive-blocked',
    label: 'Archive',
    ariaLabel: `Archive thread. ${reason}`,
    tooltip: reason,
    tone: 'neutral',
    heldBack: true,
    icon: () => <ArchiveIcon />,
    invoke: () => showBlockedToast("Can't archive yet", refusal),
  };
}

/** The lane's word for the fixed face. The full phrase stays its name. */
const LANE_FACE_LABEL: Record<LanePlace, string> = { 'in-batch': 'Landing', waiting: 'Waiting' };

/** Where the change stands in the landing lane. In the open batch it links
 *  the lane thread; waiting, it is a busy face. */
function laneAction(state: Extract<BannerState, { type: 'landing' }>): SlotAction {
  const label = LANE_FACE_LABEL[state.place];
  const laneThreadId = state.laneThreadId;
  if (state.place === 'waiting' || !laneThreadId) {
    return busyAction('landing-lane', label, 'neutral', LANE_PLACE_LABEL[state.place]);
  }
  return {
    key: 'landing-lane',
    label,
    ariaLabel: LANE_PLACE_LABEL[state.place],
    tooltip: LANE_THREAD_TIP,
    tone: 'neutral',
    icon: () => <InFlightIcon />,
    invoke: () => focusThreadOrBootstrap(laneThreadId),
  };
}

/** A request in flight: the plain verb beside a spinner, disabled. `name` says
 *  what is happening, for a screen reader and the tooltip. */
function busyAction(key: string, label: string, tone: SlotTone, name: string): SlotAction {
  return { key, label, ariaLabel: name, tooltip: name, tone, icon: () => null, invoke: null };
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
      key: STANDING_APPLY_KEY,
      dataRole: 'standing-apply',
      label: standing.label,
      tooltip: standing.tooltip,
      active: standingApplyThreadIds.value.has(focused),
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
 *  surfaces show and hide Diff together. It shows while the branch holds work.
 *  A pending change the engine emptied still reads `proposed`, so its file
 *  list is what hides Diff there (ADR 0400). */
function hasCcDiff(thread: ThreadState): boolean {
  if (thread.meta.channel !== 'claude_code') return false;
  switch (thread.meta.codingAgentChangeState.kind) {
    case 'none': return false;
    case 'unproposed': return true;
    case 'proposed': return pendingChangeForThread(thread.meta.id)?.file_count !== 0;
  }
}

function diffAction(threadId: string): HeaderActionSpec {
  return {
    key: DIFF_KEY,
    dataRole: 'thread-diff',
    label: 'Diff',
    tooltip: tooltipWithShortcut(DIFF_TOOLTIP, 'showThreadDiff'),
    icon: () => <DiffIcon />,
    render: (attrs) => <DiffButton threadId={threadId} attrs={attrs} />,
    onClick: () => openThreadDiff(threadId),
  };
}

/** Open a thread's branch diff, dropping the keyboard first. Every Diff entry
 *  point runs this: the button, its folded row and the shortcut. */
function openThreadDiff(threadId: string): void {
  blurPromptInputIfFocused();
  void viewThreadCcDiff(threadId);
}

/** The banner the composer row draws, or null when it draws the standalone
 *  pair instead: mid-turn, or with a draft in the composer, since Send then
 *  takes the banner's place. The composer and the shortcuts both decide here. */
export function composerBannerState(waitingState: WaitingState | null, hasContent: boolean): BannerState | null {
  return !hasContent && waitingState && waitingState.type !== 'canceling' ? waitingState : null;
}

/** The focused composer's banner state, read the way `PromptInput` reads it. */
function focusedComposerBannerState(): BannerState | null {
  const id = focusedThreadId.value;
  const draft = getDraft(id);
  const hasContent = composeHasContent(draft.text, draft.image_hashes.length, hasInFlightUploads(id));
  return composerBannerState(getWaitingState(), hasContent);
}

/** The Diff shortcut. It opens the focused thread's diff only when the
 *  composer row draws a Diff button. */
export function showFocusedThreadDiff(): void {
  const banner = focusedComposerBannerState();
  const members = banner ? getBannerActions(banner).situational : getStandaloneActions();
  const id = focusedThreadId.value;
  if (id && members.some((m) => m.key === 'thread-diff')) openThreadDiff(id);
}

/** The Apply shortcut: the Apply the composer row draws, or its standing apply.
 *
 *  A busy banner (an apply, discard or archive in flight) draws neither, so the
 *  press does nothing. A press already arming the standing apply is dropped,
 *  as the button drops it. */
export function applyFocusedThreadChange(): void {
  const banner = focusedComposerBannerState();
  if (banner) {
    if (banner.type === 'actions' && !banner.isArchiving) {
      void banner.actions.find((a) => a.kind === 'apply')?.invoke();
    }
    return;
  }
  const id = focusedThreadId.value;
  if (!id || armingStandingApplyThreadIds.value.has(id)) return;
  void resolveThreadActions(id).find((a) => a.kind === 'apply_when_settled')?.invoke();
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
  const activate = useTouchActivated(() => openThreadDiff(threadId));
  return (
    <button
      {...attrs}
      class="icon-btn header-icon"
      data-role="thread-diff"
      aria-label="Diff"
      data-tooltip={tooltipWithShortcut(DIFF_TOOLTIP, 'showThreadDiff')}
      data-tooltip-longpress=""
      onTouchStart={activate.onTouchStart}
      onTouchMove={activate.onTouchMove}
      onTouchCancel={activate.onTouchCancel}
      onTouchEnd={activate.onTouchEnd}
      onClick={activate.onClick}
    >
      <DiffIcon />
    </button>
  );
}

/** Diff and the standing apply as ONE icon box, for a row too narrow for both.
 *
 *  Its menu rows are the two members' own rows, the ones the ⋯ shows once they
 *  fold. So labels, handlers and the standing apply's checkbox state have one
 *  source. The face wears the overflow glyph: the Diff glyph would pass for a
 *  plain Diff button. While the standing apply is armed it also wears the armed
 *  tone, so the state stays visible on the merged control.
 *
 *  `attrs` are the composer row's measurement attributes. */
export function DiffApplyMenu({ members, attrs }: { members: readonly HeaderActionSpec[]; attrs: Record<string, string> }) {
  const armed = members.some((m) => m.key === STANDING_APPLY_KEY && m.active);
  const label = armed ? 'Diff and standing apply (armed)' : 'Diff and standing apply';
  return (
    <OverflowMenu
      ariaLabel={label}
      triggerAttrs={{ ...attrs, 'data-role': 'diff-apply-menu', ...(armed ? { 'data-tone': 'armed', 'aria-pressed': 'true' } : {}) }}
      face={{ class: 'icon-btn header-icon', label, children: <MoreIcon /> }}
      items={(ctx) => members.map((a) => renderMenuAction(a, ctx))}
    />
  );
}

/** The change action a still-working thread offers: arm a *standing apply*, or
 *  cancel the one it carries (ADR 0168 clause 5).
 *
 *  An ICON toggle, wearing the shape the follow toggle beside it already wears
 *  (`followLiveEdgeAction`). As a green pill it took over half a phone's prompt
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
    <ToggleIconButton
      {...attrs}
      tone="armed"
      pressed={armed}
      class="header-icon"
      data-role="standing-apply"
      aria-label={action.label}
      data-tooltip={tooltipWithShortcut(action.tooltip ?? action.label, 'applyChange')}
      data-tooltip-longpress=""
      onClick={() => {
        if (busy) return;
        void action.invoke();
      }}
    >
      <StandingApplyIcon armed={armed} />
    </ToggleIconButton>
  );
}
