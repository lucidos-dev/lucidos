import { useRef, useCallback, useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { changes, appliedChanges, setAsideChanges, findChangeById, threadMap, effectiveThreadStatus, isMidTurn, changesHasMore, changesLoadingMore, busyChangeIds, applyAllInProgress, showConfirm, standingApplyThreadIds, armingStandingApplySweep, disarmingAllStandingApply, settlingThreadCount } from '../../store/store';
import { applySingleChange, discardSingleChange, setAsideSingleChange, bringBackSingleChange, applyAllChanges, discardAllChanges, revertChange, loadMoreChanges, armStandingApply, disarmStandingApply, disarmAllStandingApplies, refreshChangesState, APPLY_NEW_VERSION_TOOLTIP } from '../../store/actions/chat-changes';
import { APPLY_INCOMPLETE_CONFIRM } from '../../store/actions/threadActions';
import { viewChangeDiff } from '../../store/actions/repositories';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import type { Change } from '../../api/client';
import { formatTimeAgo } from '../../utils/formatTime';
import { formatFileCount } from '../../utils/formatFileCount';
import { useDelayedFlag, useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf, useSkeleton, SkText, SkBlock } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { CommitList } from '../shared/CommitList';
import { SplitButton } from '../shared/SplitButton';
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
        // The row opens its thread on click, so the fold keeps its own clicks.
        <div class="change-row-commits" onClick={(e) => e.stopPropagation()}>
          <EventRowFoldView label={`${commits.length} commits`} body={<CommitList commits={commits} />} />
        </div>
      )}
    </>
  );
}

interface ChangeRowProps {
  change: Change;
  busy: boolean;
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
 *    flight, and the row wears its "Applying..." face.
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
function ChangeRow({ change, busy, armed, onOpen, onDiff, onDiscard, onSetAside, onApply, onStanding }: Partial<ChangeRowProps>) {
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
      class={`list-row change-row${clickable ? ' clickable' : ''}`}
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
              {change.incomplete && ' · Incomplete'}
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
        {/* An apply in flight is progress, not a blocked action, so it takes
            the one disabled face on this row. It carries no tooltip, which is
            what the disabled ban is about. */}
        {busy ? (
          <SkBlock w="4.75rem" h="2rem" round>
            <button class="action-btn action-btn-confirm change-row-primary" disabled>Applying...</button>
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

/** Apply one change from its row. Incomplete work confirms first, as the
 *  thread's own Apply does. */
async function applyFromRow(id: string): Promise<void> {
  const incomplete = findChangeById(id)?.incomplete ?? false;
  if (incomplete && !(await showConfirm(APPLY_INCOMPLETE_CONFIRM, 'Apply', { variant: 'default' }))) return;
  await applySingleChange(id);
}

export const BRING_BACK_ROW_TIP =
  'Return this change to pending, where it can be applied or discarded.';

/** A set-aside change's row: its headline, and the ways back or out. Discard
 *  is withheld while its thread works, since the engine refuses it then. */
function SetAsideRow({ change, busy, onBringBack, onDiscard }: {
  change: Change;
  busy: boolean;
  onBringBack: () => void;
  onDiscard: () => void;
}) {
  const thread = change.thread_id ? threadMap.value.get(change.thread_id) : undefined;
  const threadWorking = !!thread && isMidTurn(effectiveThreadStatus(thread));
  return (
    <div
      class={`list-row change-row${change.thread_id ? ' clickable' : ''}`}
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
            menuItems={threadWorking || busy ? [] : [{
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

export const SWEEP_TIP =
  'Apply every change that is ready, and each one still settling the moment its thread finishes.';

export const SWEEP_ARMED_TIP =
  'Armed. Each change applies the moment its thread finishes. Click to cancel every standing apply here. Anything already applying keeps going.';

/** What the bulk row offers, given the pending list, how many threads are still
 *  working, and how many carry a *standing apply*.
 *
 *  Up to three buttons: Discard All, "Apply all on settle" and Apply All. The
 *  sweep is a TOGGLE: armed, the same control cancels (ADR 0168). Everything
 *  falls out of three questions. Is there something to apply now, something to
 *  arm, and is anything armed already?
 *
 *  Pure, so the answer is testable without a render. */
export interface BulkApplyState {
  show: boolean;
  /** At least one pending change the server would apply right now. */
  canApplyNow: boolean;
  /** Threads are working, or something is armed, so the sweep toggle draws. */
  offerSweep: boolean;
  /** Something is armed, so the sweep control wears its cancel face. */
  armed: boolean;
  /** Something to apply now, or a batch running with no sweep to offer. The
   *  sweep's own request also sets the in-flight flag, and must not draw
   *  Apply All's "Applying..." beside the toggle. */
  showApplyAll: boolean;
  showDiscardAll: boolean;
}

export function bulkApplyState(
  pending: Change[],
  workingThreads: number,
  armedThreads: number,
  applyAllRunning = false,
): BulkApplyState {
  // Apply All passes incomplete work over, as the engine's batch does.
  const canApplyNow = pending.some((c) => !applyBlockedReason(c) && !c.incomplete);
  const armed = armedThreads > 0;
  // Armed with nothing left working still draws the control, or the last arm
  // would be unreachable in the window before it fires.
  const offerSweep = workingThreads > 0 || armed;
  const showDiscardAll = pending.length > 1;
  return {
    show: showDiscardAll || offerSweep,
    canApplyNow,
    offerSweep,
    armed,
    showApplyAll: canApplyNow || (applyAllRunning && !offerSweep),
    showDiscardAll,
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
  const busyIds = useSignal<Set<string>>(new Set());

  const guardedAction = useCallback((id: string, action: (id: string) => Promise<void>) => {
    if (busyIds.value.has(id)) return;
    busyIds.value = new Set([...busyIds.value, id]);
    action(id).finally(() => {
      const next = new Set(busyIds.value);
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

  // Infinite scroll: observe a sentinel at the bottom of the applied list. The
  // real scroll container is the ancestor `.content-pane-body` (it scrolls,
  // per panels/shell.css), NOT this view's `.panel-content`: an `onScroll`
  // listener on `.panel-content` never fired because that element doesn't scroll
  // (scroll events don't bubble). Rooting the observer at `.content-pane-body`
  // (mirrors NotificationsView) loads the next page as the sentinel comes into
  // view. `loadMoreChanges` self-guards against concurrent calls and the
  // no-more-pages case, so a stray intersection is harmless.
  useEffect(() => {
    const sentinel = sentinelRef.current;
    if (!sentinel || !hasMore) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting) void loadMoreChanges();
      },
      { root: sentinel.closest('.content-pane-body'), threshold: 0 },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [hasMore]);

  // Both signals load and update in lockstep (refreshChangesState and the
  // ChangesUpdated SSE both set them together), so failure on one ≈ failure
  // on both. Pending drives the spinner — its `loading` window is what the
  // user is waiting on for the next render.
  const showLoading = useDelayedLoading(pendingLoadable);

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
  const allLoaded =
    pendingLoadable.status === 'loaded' &&
    appliedLoadable.status === 'loaded' &&
    setAsideLoadable.status === 'loaded';

  return (
    <div class="panel-content protected-surface">
      <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf fill containerClass="list-rows" row={() => <ChangeRow />} />}>
        {allLoaded ? (() => {
          const pending = pendingLoadable.data;
          const applied = appliedLoadable.data;
          const setAside = setAsideLoadable.data;
          // A pending cancel holds every face unarmed. It waits for an
          // in-flight sweep, whose arm frames would otherwise flick them back.
          const cancelingAll = disarmingAllStandingApply.value;
          const armedThreads = cancelingAll
            ? 0
            : standingApplyThreadIds.value.size || (armingStandingApplySweep.value ? 1 : 0);
          const bulk = bulkApplyState(pending, settlingThreadCount.value, armedThreads, applyAllInProgress.value);
          const bulkRow = bulk.show ? (
            <div class="changes-bulk-actions">
              <div class="changes-bulk-buttons">
                {/* Discard All skips changes whose thread is still working
                    (server-side too); disable the button when none are eligible. */}
                {bulk.showDiscardAll && (
                  <button class="action-btn action-btn-danger" disabled={applyAllInProgress.value || !pending.some(c => !c.thread_unsettled)} onClick={() => void discardAllChanges()}>Discard All</button>
                )}
                {/* The sweep, as ONE control with two faces. Armed it loses the
                    green and cancels on click, the shape the per-change row and
                    the prompt-row flag icon already wear, so all three surfaces
                    read one state and all three can turn it off.

                    TWO faces, never a third. This control ARMS, so it has no
                    progress of its own to report: a press lands on the armed
                    face at once, and the `StandingApplyArmed` events hold it
                    there. Apply All's "Applying..." belongs to a batch this
                    press never starts, and wearing it flashes a narrower, faded
                    pill on the way.

                    Neither face is disabled, even mid-batch. Cancelling an
                    instruction is not the batch, and a faded control takes its
                    explaining tooltip with it (ADR 0168). A second press while
                    the request is in flight is dropped by the action. */}
                {bulk.offerSweep && (
                  bulk.armed ? (
                    <button
                      class="action-btn"
                      aria-pressed
                      data-tooltip={SWEEP_ARMED_TIP}
                      onClick={() => void disarmAllStandingApplies()}
                    >
                      ✓ Applying all on settle
                    </button>
                  ) : (
                    <button
                      class="action-btn action-btn-confirm"
                      aria-pressed={false}
                      data-tooltip={SWEEP_TIP}
                      onClick={() => void applyAllChanges(true)}
                    >
                      Apply all on settle
                    </button>
                  )
                )}
                {/* Apply All never lights up for a batch the server would reject:
                    enablement reads the same rule the per-row control and the
                    server use. With nothing appliable now the sweep toggle is
                    the whole action, so this is not drawn at all. */}
                {bulk.showApplyAll && (
                  <button
                    class="action-btn action-btn-confirm"
                    disabled={applyAllInProgress.value || !bulk.canApplyNow}
                    onClick={() => void applyAllChanges(false)}
                  >
                    {applyAllInProgress.value ? 'Applying...' : 'Apply All'}
                  </button>
                )}
              </div>
            </div>
          ) : null;
          return pending.length === 0 && applied.length === 0 && setAside.length === 0 ? (
            <>
              {bulkRow}
              <div class="empty-state">No changes</div>
            </>
          ) : (
            <>
              {bulkRow}
          {pending.map(change => {
            const busy =
              busyIds.value.has(change.id) ||
              busyChangeIds.value.has(change.id) ||
              !!change.resolving_conflict;
            // A change whose thread is mid-turn can't be applied or discarded:
            // doing so races (or yanks the worktree from) the live coding-agent
            // session. The server refuses it too (guard_change_action). What the
            // row offers instead is the standing apply.
            const armed = !cancelingAll && !!change.thread_id && standingApplyThreadIds.value.has(change.thread_id);
            return (
              <ChangeRow
                key={change.id}
                change={change}
                busy={busy}
                armed={armed}
                onOpen={() => openChangeThread(change)}
                onDiff={() => void viewChangeDiff(change)}
                onDiscard={() => guardedAction(change.id, discardSingleChange)}
                onSetAside={() => guardedAction(change.id, setAsideSingleChange)}
                onApply={() => guardedAction(change.id, applyFromRow)}
                onStanding={() => {
                  if (!change.thread_id) return;
                  void (armed
                    ? disarmStandingApply(change.thread_id)
                    : armStandingApply(change.thread_id, change.id));
                }}
              />
            );
          })}
          {setAside.length > 0 && (
            <>
              <div class="list-section-title">Set aside</div>
              {setAside.map(change => (
                <SetAsideRow
                  key={change.id}
                  change={change}
                  busy={busyIds.value.has(change.id)}
                  onBringBack={() => guardedAction(change.id, bringBackSingleChange)}
                  onDiscard={() => guardedAction(change.id, discardSingleChange)}
                />
              ))}
            </>
          )}
          {applied.length > 0 && (
            <>
              <div class="list-section-title">Recently Applied</div>
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
                          guardedAction(change.id, revertChange);
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
            </>
          )}
            </>
          );
        })() : null}
      </LoadingFade>
    </div>
  );
}
