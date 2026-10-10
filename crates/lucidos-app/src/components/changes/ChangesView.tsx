import { useRef, useCallback, useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { changes, appliedChanges, setAsideChanges, changeIsReady, threadMap, threadUnsettled, changesHasMore, changesLoadingMore, busyChangeIds, applyAllInProgress, applyAllBatch, applyPhases, showConfirm, standingApplyThreadIds, collapsedChangesSectionIds, toggleChangesSectionCollapsed, type ApplyAllBatch, type ApplyPhase, type ApplyPhaseReading } from '../../store/store';
import { applyPhaseOf, isBatchMember } from '../../store/actions/applyProgress';
import { applySingleChange, discardSingleChange, setAsideSingleChange, bringBackSingleChange, applyAllChanges, discardAllChanges, revertChange, loadMoreChanges, armStandingApply, disarmStandingApply, armStandingApplies, disarmStandingApplies, refreshChangesState, APPLY_NEW_VERSION_TOOLTIP } from '../../store/actions/chat-changes';
import { viewChangeDiff } from '../../store/actions/repositories';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import type { Change } from '../../api/client';
import { formatTimeAgo } from '../../utils/formatTime';
import { formatFileCount } from '../../utils/formatFileCount';
import { isInteractiveTarget } from '../../utils/dom';
import { useDelayedFlag, useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf, useSkeleton, SkText, SkBlock } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { CommitList } from '../shared/CommitList';
import { SplitButton } from '../shared/SplitButton';
import { SectionHeader } from '../shared/SectionHeader';
import { Disclosure } from '../shared/Disclosure';
import { ArrivalList } from '../shared/ArrivalList';
import { useArrivals } from '../shared/arrivals';
import { EventRowFoldView } from '../chat/EventRow';
import { changeCommitList, changeHeadline } from '../../store/changeHeadline';

/** A change's headline, with its commits one fold away when there are
 *  several. The same pair the change card draws. */
function ChangeHeadline({ change }: { change: Change }) {
  const commits = changeCommitList(change);
  return (
    <>
      <span class="title change-description">{changeHeadline(change)}</span>
      {commits.length > 1 && (
        // The row opens its thread on click, so the fold toggle keeps its own.
        // Only the toggle: this block spans the row, and the rest opens the thread.
        <div
          class="change-row-commits"
          onClick={(e) => { if (isInteractiveTarget(e.target)) e.stopPropagation(); }}
        >
          <EventRowFoldView label={`${commits.length} commits`} body={<CommitList commits={commits} />} />
        </div>
      )}
    </>
  );
}

interface ChangeRowProps {
  change: Change;
  /** The arrival marker's classes while the row is lit (`ArrivalList`). */
  markerClass?: string;
  /** What the action in flight is doing, or `null` when the row is idle. */
  progressLabel: string | null;
  armed: boolean;
  onOpen: () => void;
  onDiff: () => void;
  onDiscard: () => void;
  onSetAside: () => void;
  onApply: () => void;
  onStanding: () => void;
}

export const THREAD_UNSETTLED_TIP =
  'The coding agent has not finished with this thread. It is working, or waiting for something that will wake it, so wait for it to settle';

/** Why Apply is unavailable for this change, or `null` when it can be applied.
 *
 *  Both reasons are enforced server-side too (`guard_change_action` 409s, and
 *  Apply All filters the batch), so this is the UI mirror of one rule rather
 *  than a second one — the single source both the per-row button and the
 *  Apply All enablement read, so the button can't offer what the server will
 *  reject. Discard is deliberately NOT gated on the empty case: discarding is
 *  how the user resolves a change whose branch commits cancelled out. */
export function applyBlockedReason(change: Change): string | null {
  if (change.thread_unsettled) return THREAD_UNSETTLED_TIP;
  if (change.file_count === 0) return 'This change has no file changes left — discard it';
  return null;
}

export const SET_ASIDE_ROW_TIP =
  'Keep this change for later. It leaves Review and Apply All, and its thread can be archived.';

/** One action button on a pending change's row. */
export type ChangeRowAction =
  | { kind: 'standing'; label: string; tooltip: string }
  | { kind: 'discard' }
  | { kind: 'set-aside' }
  | { kind: 'apply'; label: string; tooltip?: string };

/** Which actions a pending change's row draws, and never a disabled one.
 *
 *  ADR 0168: `.action-btn:disabled` sets `pointer-events: none`, so a faded
 *  button carries a tooltip nobody can read. A control that cannot act is
 *  replaced by the one that can, or drawn not at all.
 *
 *  - Thread settling (working, or watching an event): the standing apply, and
 *    nothing else. Discard would yank the worktree from a live session, so it
 *    is not offered.
 *  - Thread parked on a QUESTION: nothing. A standing apply would drop the
 *    moment it was pressed, so offering one is the same broken control in a
 *    new coat. The row's details line says the thread has not finished.
 *  - Apply resolving merge conflicts: nothing. That apply is already in
 *    flight, and the row wears its progress face.
 *  - Nothing left in the change: Discard alone. That IS how an emptied change
 *    is resolved, and Apply is removed rather than faded.
 *  - Otherwise: Discard, Set aside and Apply, all live. Set aside is offered
 *    exactly where Discard is, as the engine's gate says (ADR 0328).
 *
 *  Pure, so the rule is one testable function rather than a pile of `disabled`
 *  expressions in the markup. */
export function changeRowActions(change: Change, armed: boolean): ChangeRowAction[] {
  if (change.thread_unsettled) {
    if (!change.thread_settling || change.resolving_conflict) return [];
    return [
      {
        kind: 'standing',
        label: armed ? '✓ Applying on settle' : 'Apply on settle',
        tooltip: armed
          ? 'Armed. This change applies when its thread finishes, and drops with a report if the thread stops on a question or fails. Click to cancel.'
          : `${THREAD_UNSETTLED_TIP}. Arm this and it applies the moment the thread finishes.`,
      },
    ];
  }
  if (change.file_count === 0) return [{ kind: 'discard' }];
  return [
    { kind: 'discard' },
    { kind: 'set-aside' },
    {
      kind: 'apply',
      label: change.requires_restart ? 'Apply*' : 'Apply',
      tooltip: change.requires_restart ? APPLY_NEW_VERSION_TOOLTIP : undefined,
    },
  ];
}

/** A row action this view runs itself and waits on. */
export type LocalRowAction = 'apply' | 'discard' | 'set-aside' | 'bring-back' | 'revert';

/** Short enough for the `.change-row-primary` slot, so Diff keeps its column.
 *  The details line spells out a conflict in full. */
const APPLY_PHASE_PROGRESS: Record<ApplyPhase, string> = {
  merging: 'Applying...',
  hardening: 'Hardening...',
  'resolving-conflict': 'Resolving...',
};

/** What a busy row's one disabled face says. An apply names its phase, so a
 *  long hardening reads as hardening.
 *
 *  An Apply All member names a phase only on evidence: an event this page saw,
 *  the served conflict flag, or the engine listing it as applying. The live
 *  stream does not say which member runs, and an unhardened member hardens
 *  only once the batch reaches it. */
export function rowProgressLabel(
  change: Change,
  local: LocalRowAction | undefined,
  phases: ReadonlyMap<string, ApplyPhaseReading>,
  batch: ApplyAllBatch | null,
): string {
  if (local === 'discard') return 'Discarding...';
  if (local === 'set-aside') return 'Setting aside...';
  const seen = change.thread_id ? phases.get(change.thread_id) : undefined;
  const unplacedMember = isBatchMember(batch, change.id)
    && !batch!.applyingChangeIds.includes(change.id)
    && !seen
    && !change.resolving_conflict;
  if (unplacedMember) return APPLY_PHASE_PROGRESS.merging;
  const reading = applyPhaseOf(change.thread_id, change, phases);
  return APPLY_PHASE_PROGRESS[reading?.phase ?? 'merging'];
}

type ApplyRowAction = Extract<ChangeRowAction, { kind: 'apply' }>;

/** How the row draws its actions. With an Apply, it is the thread's own change
 *  button: Apply as a split button, the rest behind its caret, Discard last. */
export function rowActionLayout(actions: ChangeRowAction[]):
  | { kind: 'split'; primary: ApplyRowAction; menu: ChangeRowAction[] }
  | { kind: 'flat'; buttons: ChangeRowAction[] } {
  const primary = actions.find((a): a is ApplyRowAction => a.kind === 'apply');
  if (!primary) return { kind: 'flat', buttons: actions };
  const menu = actions
    .filter((a) => a !== primary)
    .sort((a, b) => Number(a.kind === 'discard') - Number(b.kind === 'discard'));
  return { kind: 'split', primary, menu };
}

/** Self-skeletonizing pending change row: rendered with no props inside a
 *  SkeletonProvider (`<ChangeRow />`) it draws itself as a loading placeholder
 *  via the Sk* leaves; with real props it renders normally. Props are optional
 *  only to support the skeleton call; real call sites pass them all. */
function ChangeRow({ change, markerClass, progressLabel, armed, onOpen, onDiff, onDiscard, onSetAside, onApply, onStanding }: Partial<ChangeRowProps>) {
  const sk = useSkeleton();
  const layout = rowActionLayout(change ? changeRowActions(change, !!armed) : []);
  const clickable = !sk && !!change?.thread_id;
  const runAction = (kind: ChangeRowAction['kind']) => {
    if (kind === 'standing') onStanding?.();
    else if (kind === 'discard') onDiscard?.();
    else if (kind === 'set-aside') onSetAside?.();
    else onApply?.();
  };
  return (
    <div
      class={rowClass(clickable, markerClass)}
      onClick={clickable ? onOpen : undefined}
    >
      <div class="list-row-info">
        {(sk || change?.thread_title) && (
          <SkText class="list-row-label" w="11rem">{change?.thread_title}</SkText>
        )}
        {sk ? (
          <SkText class="title change-description" w="18rem" />
        ) : (
          <ChangeHeadline change={change!} />
        )}
        <SkText class="list-row-details" w="7rem">
          {change && (
            <>
              {formatFileCount(change.file_count)}
              {change.requires_restart && ' · Requires engine restart'}
              {!change.hardened && ' · Not hardened'}
              {/* No action can resolve an unsettled change, so the reason is
                  read here rather than from a tooltip on a control that is
                  not drawn. */}
              {change.resolving_conflict
                ? ' · Resolving merge conflicts'
                : change.thread_unsettled && ' · The thread has not finished'}
              {!change.resolving_conflict && change.predicted_conflict === 'conflict'
                && ' · Likely merge conflict'}
            </>
          )}
        </SkText>
      </div>
      <div class="list-row-actions">
        <SkBlock w="3rem" h="2rem" round>
          <button class="action-btn" onClick={(e) => { e.stopPropagation(); onDiff?.(); }}>Diff</button>
        </SkBlock>
        {/* An action in flight is progress, not a blocked action, so it takes
            the one disabled face on this row. It carries no tooltip, which is
            what the disabled ban is about. */}
        {progressLabel ? (
          <SkBlock w="4.75rem" h="2rem" round>
            <button class="action-btn action-btn-confirm change-row-primary" disabled>{progressLabel}</button>
          </SkBlock>
        ) : sk ? (
          <div class="change-row-primary"><SkBlock w="100%" h="2rem" round /></div>
        ) : layout.kind === 'split' ? (
          // The menu renders inside the row, so its clicks must not open the thread.
          <div class="change-row-primary" onClick={(e) => e.stopPropagation()}>
            <SplitButton
              primaryLabel={layout.primary.label}
              primaryClassName="action-btn action-btn-confirm"
              primaryTooltip={layout.primary.tooltip}
              onPrimary={() => runAction('apply')}
              caretClassName="action-btn action-btn-confirm"
              caretAriaLabel="More change actions"
              menuItems={layout.menu.map((action) => ({
                key: action.kind,
                label: rowActionLabel(action),
                className: rowActionClass(action, !!armed),
                tooltip: rowActionTooltip(action),
                onClick: () => runAction(action.kind),
              }))}
            />
          </div>
        ) : (
          layout.buttons.map((action) => (
            <button
              key={action.kind}
              class={`${rowActionClass(action, !!armed)} change-row-primary`}
              // The standing apply is a toggle, so it says so the way the
              // prompt-row icon and the bulk control do.
              aria-pressed={action.kind === 'standing' ? armed : undefined}
              data-tooltip={rowActionTooltip(action)}
              onClick={(e) => { e.stopPropagation(); runAction(action.kind); }}
            >
              {rowActionLabel(action)}
            </button>
          ))
        )}
      </div>
    </div>
  );
}

function rowClass(clickable: boolean, markerClass: string | undefined): string {
  return ['list-row change-row', clickable && 'clickable', markerClass].filter(Boolean).join(' ');
}

export function rowActionClass(action: ChangeRowAction, armed: boolean): string {
  switch (action.kind) {
    case 'discard': return 'action-btn action-btn-danger';
    case 'set-aside': return 'action-btn';
    case 'standing': return armed ? 'action-btn' : 'action-btn action-btn-confirm';
    case 'apply': return 'action-btn action-btn-confirm';
  }
}

function rowActionTooltip(action: ChangeRowAction): string | undefined {
  switch (action.kind) {
    case 'discard': return undefined;
    case 'set-aside': return SET_ASIDE_ROW_TIP;
    case 'standing':
    case 'apply': return action.tooltip;
  }
}

function rowActionLabel(action: ChangeRowAction): string {
  switch (action.kind) {
    case 'discard': return 'Discard';
    case 'set-aside': return 'Set aside';
    case 'standing':
    case 'apply': return action.label;
  }
}

export const BRING_BACK_ROW_TIP =
  'Return this change to pending, where it can be applied or discarded.';

/** A set-aside change's row: its headline, and the ways back or out. Discard
 *  is withheld while its thread is unsettled, since the engine refuses it then. */
function SetAsideRow({ change, markerClass, busy, onBringBack, onDiscard }: {
  change: Change;
  markerClass: string | undefined;
  busy: boolean;
  onBringBack: () => void;
  onDiscard: () => void;
}) {
  const thread = change.thread_id ? threadMap.value.get(change.thread_id) : undefined;
  const unsettled = !!thread && threadUnsettled(thread);
  return (
    <div
      class={rowClass(!!change.thread_id, markerClass)}
      onClick={change.thread_id ? () => openChangeThread(change) : undefined}
    >
      <div class="list-row-info">
        {change.thread_title && <span class="list-row-label">{change.thread_title}</span>}
        <ChangeHeadline change={change} />
        <span class="list-row-details">
          {formatFileCount(change.file_count)}
          {change.requires_restart && ' · Requires engine restart'}
          {change.incomplete && ' · Incomplete'}
          {` · ${formatTimeAgo(new Date(change.created_at))}`}
        </span>
      </div>
      <div class="list-row-actions">
        <button class="action-btn" onClick={(e) => { e.stopPropagation(); void viewChangeDiff(change); }}>Diff</button>
        {/* The menu renders inside the row, so its clicks must not open the thread. */}
        <div class="change-row-primary" onClick={(e) => e.stopPropagation()}>
          <SplitButton
            primaryLabel="Bring back"
            primaryClassName="action-btn"
            primaryTooltip={busy ? undefined : BRING_BACK_ROW_TIP}
            primaryDisabled={busy}
            onPrimary={onBringBack}
            caretClassName="action-btn"
            caretAriaLabel="More set-aside actions"
            menuItems={unsettled || busy ? [] : [{
              key: 'discard',
              label: 'Discard',
              className: 'action-btn action-btn-danger',
              onClick: onDiscard,
            }]}
          />
        </div>
      </div>
    </div>
  );
}

export const SETTLE_ALL_TIP =
  'Apply each change here the moment its thread finishes.';

export const SETTLE_ALL_ARMED_TIP =
  'Armed. Each change here applies the moment its thread finishes. Click to cancel. Anything already applying keeps going.';

/** How the pending list splits into its two sections, and what each section's
 *  bulk row offers. Pure, so the answer is testable without a render.
 *
 *  **Ready** holds changes whose thread has finished. Its row offers Discard
 *  All and Apply All. **Not finished** holds the rest: a thread settling, or
 *  parked on a question. Its row
 *  offers only "Apply all on settle", a TOGGLE over the settling changes it
 *  lists (ADR 0168). Discard never reaches it. */
export interface PendingSections {
  ready: Change[];
  notFinished: Change[];
  /** At least one Ready change the server would apply right now. */
  canApplyNow: boolean;
  /** Something to apply now, or a batch still running. */
  showApplyAll: boolean;
  showDiscardAll: boolean;
  /** The settling changes "Apply all on settle" arms. A parked thread's arm
   *  would drop at once, and a merge being resolved is already applying. */
  armable: Change[];
  /** Every armable change is armed, so the toggle wears its cancel face. */
  armed: boolean;
}

export function pendingSections(
  pending: Change[],
  armedThreadIds: ReadonlySet<string>,
  applyAllRunning = false,
): PendingSections {
  const ready = pending.filter(changeIsReady);
  const notFinished = pending.filter((c) => !changeIsReady(c));
  const canApplyNow = ready.some((c) => !applyBlockedReason(c));
  const armable = notFinished.filter((c) => c.thread_settling && !c.resolving_conflict && c.thread_id);
  return {
    ready,
    notFinished,
    canApplyNow,
    showApplyAll: canApplyNow || applyAllRunning,
    showDiscardAll: ready.length > 1,
    armable,
    armed: armable.length > 0 && armable.every((c) => armedThreadIds.has(c.thread_id!)),
  };
}

/** Open the thread that produced a change, landing on the turn where the change
 *  originated (its `ChangeProposed` is stamped with `data-change-id` on that
 *  exchange) rather than the bottom of the thread — the change isn't necessarily
 *  the last turn. Uses focusThreadOrBootstrap so a thread outside the loaded
 *  window (old archived row, cross-workspace link) still opens. Exported for the
 *  unit test. */
export function openChangeThread(change: Change): void {
  if (!change.thread_id) return;
  focusThreadOrBootstrap(change.thread_id, { targetChangeId: change.id });
}

export function ChangesView() {
  const sentinelRef = useRef<HTMLDivElement>(null);
  usePanelRefresh('changes', refreshChangesState);
  const busyIds = useSignal<Map<string, LocalRowAction>>(new Map());

  const guardedAction = useCallback((id: string, kind: LocalRowAction, action: (id: string) => Promise<void>) => {
    if (busyIds.value.has(id)) return;
    busyIds.value = new Map([...busyIds.value, [id, kind]]);
    action(id).finally(() => {
      const next = new Map(busyIds.value);
      next.delete(id);
      busyIds.value = next;
    });
  }, []);

  const pendingLoadable = changes.value;
  const appliedLoadable = appliedChanges.value;
  const setAsideLoadable = setAsideChanges.value;
  const hasMore = changesHasMore.value;
  const loadingMore = changesLoadingMore.value;
  const showLoadingMore = useDelayedFlag(loadingMore);
  const readyCollapsed = collapsedChangesSectionIds.value.has('ready');
  const notFinishedCollapsed = collapsedChangesSectionIds.value.has('not-finished');
  const setAsideCollapsed = collapsedChangesSectionIds.value.has('set-aside');
  const appliedCollapsed = collapsedChangesSectionIds.value.has('applied');

  // Infinite scroll: observe a sentinel at the bottom of the applied list. The
  // real scroll container is the ancestor `.content-pane-body` (it scrolls,
  // per panels/shell.css), NOT this view's `.panel-content`: an `onScroll`
  // listener on `.panel-content` never fired because that element doesn't scroll
  // (scroll events don't bubble). Rooting the observer at `.content-pane-body`
  // (mirrors NotificationsView) loads the next page as the sentinel comes into
  // view. `loadMoreChanges` self-guards against concurrent calls and the
  // no-more-pages case, so a stray intersection is harmless. The sentinel
  // lives in the collapsible Recently applied section, so it re-observes
  // whenever that section opens and mounts a fresh one.
  useEffect(() => {
    const sentinel = sentinelRef.current;
    if (!sentinel || !hasMore || appliedCollapsed) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting) void loadMoreChanges();
      },
      { root: sentinel.closest('.content-pane-body'), threshold: 0 },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [hasMore, appliedCollapsed]);

  // Both signals load and update in lockstep (refreshChangesState and the
  // ChangesUpdated SSE both set them together), so failure on one ≈ failure
  // on both. Pending drives the spinner — its `loading` window is what the
  // user is waiting on for the next render.
  const showLoading = useDelayedLoading(pendingLoadable);

  const allLoaded =
    pendingLoadable.status === 'loaded' &&
    appliedLoadable.status === 'loaded' &&
    setAsideLoadable.status === 'loaded';
  const armedThreadIds = standingApplyThreadIds.value;
  const sections = pendingLoadable.status === 'loaded'
    ? pendingSections(pendingLoadable.data, armedThreadIds, applyAllInProgress.value)
    : null;
  const ids = (list: Change[] | undefined) => (allLoaded && list ? list.map(c => c.id) : null);
  // A change that turns ready arrives in Ready. A change that drops back to
  // Not finished was already here, so Not finished lights only a new change.
  const readyArrived = useArrivals(ids(sections?.ready));
  const pendingArrived = useArrivals(ids(pendingLoadable.status === 'loaded' ? pendingLoadable.data : undefined));
  const setAsideArrived = useArrivals(ids(setAsideLoadable.status === 'loaded' ? setAsideLoadable.data : undefined));

  if (pendingLoadable.status === 'failed' || appliedLoadable.status === 'failed' || setAsideLoadable.status === 'failed') {
    const err = pendingLoadable.status === 'failed' ? pendingLoadable.error
              : appliedLoadable.status === 'failed' ? appliedLoadable.error
              : setAsideLoadable.status === 'failed' ? setAsideLoadable.error
              : 'Unknown error';
    return (
      <div class="panel-content protected-surface">
        <LoadableError noun="changes" error={err} />
      </div>
    );
  }
  return (
    <div class="panel-content protected-surface list-rows-divided">
      <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf fill containerClass="list-rows" row={() => <ChangeRow />} />}>
        {allLoaded && sections ? (() => {
          const pending = pendingLoadable.data;
          const applied = appliedLoadable.data;
          const setAside = setAsideLoadable.data;
          const renderRow = (change: Change, markerClass: string | undefined) => {
            const local = busyIds.value.get(change.id);
            const busy = !!local || busyChangeIds.value.has(change.id) || !!change.resolving_conflict;
            const progressLabel = busy
              ? rowProgressLabel(change, local, applyPhases.value, applyAllBatch.value)
              : null;
            // A change whose thread is mid-turn can't be applied or discarded:
            // doing so races (or yanks the worktree from) the live coding-agent
            // session. The server refuses it too (guard_change_action). What the
            // row offers instead is the standing apply.
            const armed = !!change.thread_id && armedThreadIds.has(change.thread_id);
            return (
              <ChangeRow
                change={change}
                markerClass={markerClass}
                progressLabel={progressLabel}
                armed={armed}
                onOpen={() => openChangeThread(change)}
                onDiff={() => void viewChangeDiff(change)}
                onDiscard={() => guardedAction(change.id, 'discard', discardSingleChange)}
                onSetAside={() => guardedAction(change.id, 'set-aside', setAsideSingleChange)}
                onApply={() => guardedAction(change.id, 'apply', applySingleChange)}
                onStanding={() => {
                  if (!change.thread_id) return;
                  void (armed
                    ? disarmStandingApply(change.thread_id)
                    : armStandingApply(change.thread_id, change.id));
                }}
              />
            );
          };
          return pending.length === 0 && applied.length === 0 && setAside.length === 0 ? (
            <div class="empty-state">No changes</div>
          ) : (
            <>
          {/* Each section rolls in with its first row and out with its last. */}
          <Disclosure open={sections.ready.length > 0}>
            <SectionHeader
              title="Ready"
              count={sections.ready.length}
              collapsed={readyCollapsed}
              onToggle={() => toggleChangesSectionCollapsed('ready')}
            />
            <Disclosure open={!readyCollapsed}>
              {(sections.showDiscardAll || sections.showApplyAll) && (
                <div class="changes-bulk-actions">
                  <div class="changes-bulk-buttons">
                    {sections.showDiscardAll && (
                      <button class="action-btn action-btn-danger" disabled={applyAllInProgress.value} onClick={() => void discardAllChanges()}>Discard All</button>
                    )}
                    {/* Apply All never lights up for a batch the server would
                        reject: enablement reads the same rule the per-row
                        control and the server use. */}
                    {sections.showApplyAll && (
                      <button
                        class="action-btn action-btn-confirm"
                        disabled={applyAllInProgress.value || !sections.canApplyNow}
                        onClick={() => void applyAllChanges()}
                      >
                        {applyAllInProgress.value ? 'Applying...' : 'Apply All'}
                      </button>
                    )}
                  </div>
                </div>
              )}
              <ArrivalList items={sections.ready} keyOf={(c) => c.id} arrived={readyArrived}>
                {renderRow}
              </ArrivalList>
            </Disclosure>
          </Disclosure>
          <Disclosure open={sections.notFinished.length > 0}>
            <SectionHeader
              title="Not finished"
              count={sections.notFinished.length}
              collapsed={notFinishedCollapsed}
              onToggle={() => toggleChangesSectionCollapsed('not-finished')}
            />
            <Disclosure open={!notFinishedCollapsed}>
              {/* ONE control with two faces. Armed it loses the green and
                  cancels on click, the shape the per-change row and the
                  prompt-row flag icon already wear, so all three surfaces read
                  one state and all three can turn it off. Neither face is
                  disabled: a faded control takes its explaining tooltip with
                  it (ADR 0168). */}
              {sections.armable.length > 0 && (
                <div class="changes-bulk-actions">
                  <div class="changes-bulk-buttons">
                    {sections.armed ? (
                      <button
                        class="action-btn"
                        aria-pressed
                        data-tooltip={SETTLE_ALL_ARMED_TIP}
                        onClick={() => void disarmStandingApplies(sections.armable.map((c) => c.thread_id!))}
                      >
                        ✓ Applying all on settle
                      </button>
                    ) : (
                      <button
                        class="action-btn action-btn-confirm"
                        aria-pressed={false}
                        data-tooltip={SETTLE_ALL_TIP}
                        onClick={() => void armStandingApplies(
                          sections.armable.filter((c) => !armedThreadIds.has(c.thread_id!)),
                        )}
                      >
                        Apply all on settle
                      </button>
                    )}
                  </div>
                </div>
              )}
              <ArrivalList items={sections.notFinished} keyOf={(c) => c.id} arrived={pendingArrived}>
                {renderRow}
              </ArrivalList>
            </Disclosure>
          </Disclosure>
          <Disclosure open={setAside.length > 0}>
            <SectionHeader
              title="Set aside"
              count={setAside.length}
              collapsed={setAsideCollapsed}
              onToggle={() => toggleChangesSectionCollapsed('set-aside')}
            />
            <Disclosure open={!setAsideCollapsed}>
              <ArrivalList items={setAside} keyOf={(c) => c.id} arrived={setAsideArrived}>
                {(change, markerClass) => (
                  <SetAsideRow
                    change={change}
                    markerClass={markerClass}
                    busy={busyIds.value.has(change.id)}
                    onBringBack={() => guardedAction(change.id, 'bring-back', bringBackSingleChange)}
                    onDiscard={() => guardedAction(change.id, 'discard', discardSingleChange)}
                  />
                )}
              </ArrivalList>
            </Disclosure>
          </Disclosure>
          {applied.length > 0 && (
            <>
              <SectionHeader
                title="Recently applied"
                count={applied.length}
                collapsed={appliedCollapsed}
                onToggle={() => toggleChangesSectionCollapsed('applied')}
              />
              <Disclosure open={!appliedCollapsed}>
                {applied.map(change => (
                  <div
                    class={`list-row change-row${change.thread_id ? ' clickable' : ''}`}
                    key={change.id}
                    style="opacity: 0.7"
                    onClick={change.thread_id ? () => openChangeThread(change) : undefined}
                  >
                    <div class="list-row-info">
                      {change.thread_title && <span class="list-row-label">{change.thread_title}</span>}
                      <ChangeHeadline change={change} />
                      <span class="list-row-details">
                        {formatFileCount(change.file_count)}
                        {change.requires_restart && ' · Requires engine restart'}
                        {change.resolved_at && ` · ${formatTimeAgo(new Date(change.resolved_at))}`}
                      </span>
                    </div>
                    <div class="list-row-actions">
                      {change.pre_merge_sha && (
                        <button class="action-btn" onClick={(e) => { e.stopPropagation(); void viewChangeDiff(change); }}>Diff</button>
                      )}
                      {change.status === 'applied' ? (
                        <button class="action-btn action-btn-danger change-row-primary" disabled={busyIds.value.has(change.id)} onClick={async (e) => {
                          e.stopPropagation();
                          if (await showConfirm('Revert this change? Any later applied changes that touch the same files may conflict.', 'Revert', { variant: 'default' })) {
                            guardedAction(change.id, 'revert', revertChange);
                          }
                        }}>Revert</button>
                      ) : (
                        <span class="list-row-details" style="font-size: var(--font-size-md)">Reverted</span>
                      )}
                    </div>
                  </div>
                ))}
                {/* The next page's rows, drawn above the sentinel that asked for them. */}
                <LoadingFade showSkeleton={showLoadingMore} skeleton={<ListSkeletonOf count={2} containerClass="list-rows" row={() => <ChangeRow />} />}>
                  {null}
                </LoadingFade>
                {hasMore && (
                  <div ref={sentinelRef} class="dropdown-panel-loading-more" style="opacity: 0.4">
                    {!loadingMore && 'Scroll for more'}
                  </div>
                )}
              </Disclosure>
            </>
          )}
            </>
          );
        })() : null}
      </LoadingFade>
    </div>
  );
}
