import type { ComponentChildren } from 'preact';
import { threadMap, focusedThreadId, getThreadDisplaySection, applyingNowThreadIds, applyingChangeThreadIds, discardingCCThreadIds, cancelingThreadIds, effectiveThreadStatus, isMidTurn, standingApplyThreadIds, armingStandingApplyThreadIds, pendingChangeForThread, landingLane, lanePlace, type LanePlace } from '../../store/store';
import { LANE_PLACE_LABEL, LANE_THREAD_TIP } from '../../store/actions/landingLane';
import { archiveRequestInFlight } from '../../store/rowIntents';
import { resolveThreadActions, type TaggedAction } from '../../store/actions/threadActions';
import { focusThreadOrBootstrap, handleArchiveThread, showBlockedToast } from '../../store/actions/threads';
import { BLOCKER_REASON, isDescendantBlocker, type BlockedRefusal } from '../../store/actions/blockerCopy';
import { exitItems, threadBlocker } from '../../store/actions/threadBlockers';
import type { ThreadState } from '../../store/thread-events';
import { viewThreadCcDiff } from '../../store/actions/repositories';
import { ToggleIconButton } from '../shared/ToggleIconButton';
import { ArchiveIcon, CheckIcon, DiffIcon, InFlightIcon, MoreIcon, HoldIcon, StandingApplyIcon, TrashIcon } from '../shared/icons';
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

export const DIFF_APPLY_MERGE = {
  keys: [DIFF_KEY, STANDING_APPLY_KEY],
  foldKey: 'diff-and-standing-apply',
} as const;

const BANNER_CLOSE_KINDS: ReadonlySet<string> = new Set(['discard', 'apply', 'hold', 'archive']);

type WaitingState =
  | { type: 'applying' }
  | { type: 'landing'; place: LanePlace; laneThreadId: string | null; threadId: string; showDiff: boolean }
  | { type: 'discarding' }
  | { type: 'canceling'; threadId: string; isCanceling: boolean }
  | {
      type: 'actions';
      actions: TaggedAction[];
      threadId: string;
      isArchiving: boolean;
      showDiff: boolean;
      blockedArchive?: BlockedRefusal;
    };

export type BannerState = Exclude<WaitingState, { type: 'canceling' }>;

export function getWaitingState(): WaitingState | null {
  const focused = focusedThreadId.value;
  if (!focused) return null;

  const thread = threadMap.value.get(focused);
  if (!thread) return null;

  if (applyingNowThreadIds.value.has(focused) && !isMidTurn(effectiveThreadStatus(thread))) {
    return { type: 'applying' };
  }

  if (discardingCCThreadIds.value.has(focused)) return { type: 'discarding' };

  if (archiveRequestInFlight(focused)) {
    return { type: 'actions', actions: [], threadId: focused, isArchiving: true, showDiff: false };
  }

  const status = effectiveThreadStatus(thread);

  if (isMidTurn(status)) {
    return {
      type: 'canceling',
      threadId: focused,
      isCanceling: cancelingThreadIds.value.has(focused),
    };
  }

  const lane = landingLane.value;
  const place = lanePlace(lane, pendingChangeForThread(focused)?.id);
  if (place) {
    return {
      type: 'landing',
      place,
      laneThreadId: lane.open_batch?.lane_thread_id ?? null,
      threadId: focused,
      showDiff: hasCcDiff(thread),
    };
  }

  if (applyingChangeThreadIds.value.has(focused)) return { type: 'applying' };

  const actions = resolveThreadActions(focused).filter((a) => BANNER_CLOSE_KINDS.has(a.kind));
  const archived = getThreadDisplaySection(thread) === 'archive';
  const blockedArchive = actions.length === 0 && !archived ? subThreadBlocker(focused) : undefined;
  if (actions.length === 0 && !archived && !blockedArchive && exitItems(focused).archive === 'enabled') {
    actions.push(archiveExitAction(focused));
  }
  if (actions.length === 0 && !blockedArchive) return null;

  const showDiff = hasCcDiff(thread);

  return {
    type: 'actions',
    actions,
    threadId: focused,
    isArchiving: false,
    showDiff,
    blockedArchive,
  };
}

function archiveExitAction(threadId: string): TaggedAction {
  return { kind: 'archive', category: 'close', label: 'Archive', invoke: async () => { await handleArchiveThread(threadId); } };
}

function subThreadBlocker(threadId: string): BlockedRefusal | undefined {
  const { blocker, subThreads } = threadBlocker(threadId);
  return blocker !== 'none' && isDescendantBlocker(blocker)
    ? { blocker, subThreadId: subThreads[0]?.thread.meta.id ?? null }
    : undefined;
}

const ACTION_ICON: Record<string, () => ComponentChildren> = {
  apply: () => <CheckIcon />,
  discard: () => <TrashIcon />,
  hold: () => <HoldIcon />,
  archive: () => <ArchiveIcon />,
};

export interface BannerActions {
  situational: HeaderActionSpec[];
  slot: SlotOccupant;
}

export function getBannerActions(state: BannerState): BannerActions {
  if (state.type === 'applying') return alone(busyAction('applying', 'Apply', 'confirm', 'Applying'));
  if (state.type === 'landing') {
    return {
      situational: state.showDiff ? [diffAction(state.threadId)] : [],
      slot: { face: laneAction(state), menu: [], menuLabel: '' },
    };
  }
  if (state.type === 'discarding') return alone(busyAction('discarding', 'Discard', 'danger', 'Discarding'));
  if (state.isArchiving) return alone(busyAction('archiving', 'Archive', 'neutral', 'Archiving'));

  return {
    situational: state.showDiff ? [diffAction(state.threadId)] : [],
    slot: closeSlot(state),
  };
}

function alone(face: SlotAction): BannerActions {
  return { situational: [], slot: { face, menu: [], menuLabel: '' } };
}

const FACE_ORDER = ['apply', 'archive', 'archive-blocked', 'hold', 'discard'];

function closeSlot(state: Extract<BannerState, { type: 'actions' }>): SlotOccupant {
  const [face, ...menu] = [
    ...state.actions.map(closeSetAction),
    ...(state.blockedArchive ? [blockedArchiveAction(state.blockedArchive)] : []),
  ].sort((a, b) => FACE_ORDER.indexOf(a.key) - FACE_ORDER.indexOf(b.key));
  const menuLabel = face.key === 'apply' ? 'More change actions' : 'More close actions';
  return { face, menu, menuLabel };
}

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

const LANE_FACE_LABEL: Record<LanePlace, string> = { 'in-batch': 'Landing', waiting: 'Waiting' };

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

function busyAction(key: string, label: string, tone: SlotTone, name: string): SlotAction {
  return { key, label, ariaLabel: name, tooltip: name, tone, icon: () => null, invoke: null };
}

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

function openThreadDiff(threadId: string): void {
  blurPromptInputIfFocused();
  void viewThreadCcDiff(threadId);
}

export function composerBannerState(waitingState: WaitingState | null, hasContent: boolean): BannerState | null {
  return !hasContent && waitingState && waitingState.type !== 'canceling' ? waitingState : null;
}

function focusedComposerBannerState(): BannerState | null {
  const id = focusedThreadId.value;
  const draft = getDraft(id);
  const hasContent = composeHasContent(draft.text, draft.image_hashes.length, hasInFlightUploads(id));
  return composerBannerState(getWaitingState(), hasContent);
}

export function showFocusedThreadDiff(): void {
  const banner = focusedComposerBannerState();
  const members = banner ? getBannerActions(banner).situational : getStandaloneActions();
  const id = focusedThreadId.value;
  if (id && members.some((m) => m.key === 'thread-diff')) openThreadDiff(id);
}

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
