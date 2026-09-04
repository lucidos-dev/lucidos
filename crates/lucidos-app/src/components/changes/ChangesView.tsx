import { Fragment } from 'preact';
import { useRef, useCallback, useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { changes, appliedChanges, changesHasMore, changesLoadingMore, busyChangeIds, applyAllInProgress, showConfirm, standingApplyThreadIds, workingThreadCount } from '../../store/store';
import { applySingleChange, discardSingleChange, applyAllChanges, discardAllChanges, revertChange, loadMoreChanges, armStandingApply, disarmStandingApply, disarmAllStandingApplies } from '../../store/actions/chat-changes';
import { viewChangeDiff } from '../../store/actions/repositories';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import type { Change } from '../../api/client';
import { formatTimeAgo } from '../../utils/formatTime';
import { formatFileCount } from '../../utils/formatFileCount';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf, useSkeleton, SkText, SkBlock } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';

/** Render a change description, preserving line breaks. */
function ChangeDescription({ description }: { description: string }) {
  const lines = (description || 'Claude Code changes').split('\n');
  return (
    <span class="title change-description">
      {lines.map((line, i) => (
        <Fragment key={i}>{line}{i < lines.length - 1 && <br />}</Fragment>
      ))}
    </span>
  );
}

interface ChangeRowProps {
  change: Change;
  busy: boolean;
  armed: boolean;
  onOpen: () => void;
  onDiff: () => void;
  onDiscard: () => void;
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

/** One action button on a pending change's row. */
export type ChangeRowAction =
  | { kind: 'standing'; label: string; tooltip: string }
  | { kind: 'discard' }
  | { kind: 'apply'; label: string; tooltip?: string };

/** Which actions a pending change's row draws, and never a disabled one.
 *
 *  ADR 0168: `.action-btn:disabled` sets `pointer-events: none`, so a faded
 *  button carries a tooltip nobody can read. A control that cannot act is
 *  replaced by the one that can, or drawn not at all.
 *
 *  - Thread still working: the standing apply, and nothing else. Discard would
 *    yank the worktree from a live session, so it is not offered.
 *  - Thread unsettled but PARKED: nothing. A standing apply would drop the
 *    moment it was pressed, so offering one is the same broken control in a
 *    new coat. The row's details line says the thread has not finished.
 *  - Nothing left in the change: Discard alone. That IS how an emptied change
 *    is resolved, and Apply is removed rather than faded.
 *  - Otherwise: Discard and Apply, both live.
 *
 *  Pure, so the rule is one testable function rather than a pile of `disabled`
 *  expressions in the markup. */
export function changeRowActions(change: Change, armed: boolean): ChangeRowAction[] {
  if (change.thread_unsettled) {
    if (!change.thread_working) return [];
    return [
      {
        kind: 'standing',
        label: armed ? '✓ Applying as it settles' : 'Apply as it settles',
        tooltip: armed
          ? 'Armed. This change applies when its thread finishes, and drops with a report if the thread parks or fails. Click to cancel.'
          : `${THREAD_UNSETTLED_TIP}. Arm this and it applies the moment the thread finishes.`,
      },
    ];
  }
  if (change.file_count === 0) return [{ kind: 'discard' }];
  return [
    { kind: 'discard' },
    {
      kind: 'apply',
      label: change.requires_restart ? 'Apply*' : 'Apply',
      tooltip: change.requires_restart
        ? 'Engine restart required for these changes to be applied correctly. You will be prompted to restart'
        : undefined,
    },
  ];
}

/** Self-skeletonizing pending change row: rendered with no props inside a
 *  SkeletonProvider (`<ChangeRow />`) it draws itself as a loading placeholder
 *  via the Sk* leaves; with real props it renders normally. Props are optional
 *  only to support the skeleton call; real call sites pass them all. */
function ChangeRow({ change, busy, armed, onOpen, onDiff, onDiscard, onApply, onStanding }: Partial<ChangeRowProps>) {
  const sk = useSkeleton();
  const actions = change ? changeRowActions(change, !!armed) : [];
  const clickable = !sk && !!change?.thread_id;
  const runAction = (kind: ChangeRowAction['kind']) => {
    if (kind === 'standing') onStanding?.();
    else if (kind === 'discard') onDiscard?.();
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
          <ChangeDescription description={change!.description} />
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
              {change.thread_unsettled && ' · The thread has not finished'}
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
            <button class="action-btn action-btn-confirm" disabled>Applying...</button>
          </SkBlock>
        ) : sk ? (
          <>
            <SkBlock w="4.25rem" h="2rem" round />
            <SkBlock w="3.75rem" h="2rem" round />
          </>
        ) : (
          actions.map((action) => (
            <button
              key={action.kind}
              class={
                action.kind === 'discard'
                  ? 'action-btn action-btn-danger'
                  : action.kind === 'standing' && armed
                    ? 'action-btn'
                    : 'action-btn action-btn-confirm'
              }
              // The standing apply is a toggle, so it says so the way the
              // prompt-row icon and the bulk control do.
              aria-pressed={action.kind === 'standing' ? armed : undefined}
              data-tooltip={action.kind === 'discard' ? undefined : action.tooltip}
              onClick={(e) => { e.stopPropagation(); runAction(action.kind); }}
            >
              {action.kind === 'discard' ? 'Discard' : action.label}
            </button>
          ))
        )}
      </div>
    </div>
  );
}

export const SWEEP_ONLY_TIP =
  'Nothing can be applied yet. Arm this and every thread still working applies its change the moment it finishes.';

export const SWEEP_ARMED_TIP =
  'Armed. Each change applies the moment its thread finishes. Click to cancel every standing apply here. Anything already applying keeps going.';

/** What the bulk row offers, given the pending list, how many threads are still
 *  working, and how many carry a *standing apply*.
 *
 *  ADR 0168 gives Apply All a "Keep going as the rest settle" checkbox. The
 *  button reads "Apply as they settle" when nothing is pending, and it is a
 *  TOGGLE: armed, the same control cancels. Everything falls out of three
 *  questions. Is there something to apply now, something to arm, and is
 *  anything armed already?
 *
 *  Pure, so the answer is testable without a render. */
export interface BulkApplyState {
  show: boolean;
  /** At least one pending change the server would apply right now. */
  canApplyNow: boolean;
  /** Nothing to apply now, but threads are working, so arming IS the action. */
  sweepOnly: boolean;
  /** Something is armed, so the sweep control wears its cancel face. */
  armed: boolean;
  /** Both, so the checkbox is a real choice. Withdrawn once armed: the toggle
   *  says the same thing and can turn it off, which a checkbox cannot. */
  offerKeepGoing: boolean;
  showDiscardAll: boolean;
}

export function bulkApplyState(
  pending: Change[],
  workingThreads: number,
  armedThreads: number,
): BulkApplyState {
  const canApplyNow = pending.some((c) => !applyBlockedReason(c));
  const armed = armedThreads > 0;
  // Armed with nothing left working still draws the control, or the last arm
  // would be unreachable in the window before it fires.
  const sweepOnly = !canApplyNow && (workingThreads > 0 || armed);
  const offerKeepGoing = canApplyNow && workingThreads > 0 && !armed;
  const showDiscardAll = pending.length > 1;
  return {
    show: showDiscardAll || sweepOnly || offerKeepGoing || armed,
    canApplyNow,
    sweepOnly,
    armed,
    offerKeepGoing,
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

  // The checkbox is a per-view choice, not a preference: it modifies the press
  // the user is about to make and nothing beyond it.
  const keepGoing = useSignal(false);

  const pendingLoadable = changes.value;
  const appliedLoadable = appliedChanges.value;
  const hasMore = changesHasMore.value;
  const loadingMore = changesLoadingMore.value;

  // Infinite scroll: observe a sentinel at the bottom of the applied list. The
  // real scroll container is the ancestor `.content-pane-body` (overflow-y: auto
  // in panels/shell.css), NOT this view's `.panel-content` — an `onScroll`
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

  if (pendingLoadable.status === 'failed' || appliedLoadable.status === 'failed') {
    const err = pendingLoadable.status === 'failed' ? pendingLoadable.error
              : appliedLoadable.status === 'failed' ? appliedLoadable.error
              : 'Unknown error';
    return (
      <div class="panel-content">
        <LoadableError noun="changes" error={err} />
      </div>
    );
  }
  const bothLoaded = pendingLoadable.status === 'loaded' && appliedLoadable.status === 'loaded';

  return (
    <div class="panel-content">
      <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf fill containerClass="list-rows" row={() => <ChangeRow />} />}>
        {bothLoaded ? (() => {
          const pending = pendingLoadable.data;
          const applied = appliedLoadable.data;
          const bulk = bulkApplyState(pending, workingThreadCount.value, standingApplyThreadIds.value.size);
          const bulkRow = bulk.show ? (
            <div class="changes-bulk-actions">
              {/* Discard All skips changes whose thread is still working
                  (server-side too); disable the button when none are eligible. */}
              {bulk.showDiscardAll && (
                <button class="action-btn action-btn-danger" disabled={applyAllInProgress.value || !pending.some(c => !c.thread_unsettled)} onClick={() => void discardAllChanges()}>Discard All</button>
              )}
              {/* "Keep going as the rest settle": the sweep. Everything
                  pending, plus everything still working, as each one lands
                  (ADR 0168). Withdrawn once armed, where the toggle beside it
                  says the same thing and can also turn it off. */}
              {bulk.offerKeepGoing && (
                <label class="changes-keep-going">
                  <input
                    type="checkbox"
                    checked={keepGoing.value}
                    onChange={(e) => { keepGoing.value = (e.currentTarget as HTMLInputElement).checked; }}
                  />
                  Keep going as the rest settle
                </label>
              )}
              {/* The sweep, as ONE control with two faces. Armed it loses the
                  green and cancels on click, the shape the per-change row and
                  the prompt-row flag icon already wear, so all three surfaces
                  read one state and all three can turn it off.

                  The armed face is never disabled, even mid-batch: cancelling
                  an instruction is not the batch, and a faded control takes its
                  explaining tooltip with it (ADR 0168). A second tap while the
                  request is in flight is dropped by the action instead. */}
              {(bulk.sweepOnly || bulk.armed) && (
                bulk.armed ? (
                  <button
                    class="action-btn"
                    aria-pressed
                    data-tooltip={SWEEP_ARMED_TIP}
                    onClick={() => void disarmAllStandingApplies()}
                  >
                    ✓ Applying as they settle
                  </button>
                ) : (
                  <button
                    class="action-btn action-btn-confirm"
                    aria-pressed={false}
                    disabled={applyAllInProgress.value}
                    // An apply in flight is progress, not a blocked action, so
                    // it takes the one disabled face here and carries NO
                    // tooltip. `.action-btn:disabled` sets `pointer-events:
                    // none` and would put this one out of reach, which is what
                    // the disabled ban is about.
                    data-tooltip={applyAllInProgress.value ? undefined : SWEEP_ONLY_TIP}
                    onClick={() => void applyAllChanges(true)}
                  >
                    {applyAllInProgress.value ? 'Applying...' : 'Apply as they settle'}
                  </button>
                )
              )}
              {/* Apply All never lights up for a batch the server would reject:
                  enablement reads the same rule the per-row control and the
                  server use. With nothing appliable now the sweep toggle above
                  is the whole action, so this is not drawn at all. */}
              {!bulk.sweepOnly && (
                <button
                  class="action-btn action-btn-confirm"
                  disabled={applyAllInProgress.value || !bulk.canApplyNow}
                  onClick={() => void applyAllChanges(keepGoing.value)}
                >
                  {applyAllInProgress.value ? 'Applying...' : 'Apply All'}
                </button>
              )}
            </div>
          ) : null;
          return pending.length === 0 && applied.length === 0 ? (
            <>
              {bulkRow}
              <div class="empty-state">No changes</div>
            </>
          ) : (
            <>
              {bulkRow}
          {pending.map(change => {
            const busy = busyIds.value.has(change.id) || busyChangeIds.value.has(change.id);
            // A change whose thread is mid-turn can't be applied or discarded:
            // doing so races (or yanks the worktree from) the live coding-agent
            // session. The server refuses it too (guard_change_action). What the
            // row offers instead is the standing apply.
            const armed = !!change.thread_id && standingApplyThreadIds.value.has(change.thread_id);
            return (
              <ChangeRow
                key={change.id}
                change={change}
                busy={busy}
                armed={armed}
                onOpen={() => openChangeThread(change)}
                onDiff={() => void viewChangeDiff(change)}
                onDiscard={() => guardedAction(change.id, discardSingleChange)}
                onApply={() => guardedAction(change.id, applySingleChange)}
                onStanding={() => {
                  if (!change.thread_id) return;
                  void (armed
                    ? disarmStandingApply(change.thread_id)
                    : armStandingApply(change.thread_id, change.id));
                }}
              />
            );
          })}
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
                    <ChangeDescription description={change.description} />
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
                      <button class="action-btn action-btn-danger" disabled={busyIds.value.has(change.id)} onClick={async (e) => {
                        e.stopPropagation();
                        if (await showConfirm('Revert this change? Any later applied changes that touch the same files may conflict.', 'Revert')) {
                          guardedAction(change.id, revertChange);
                        }
                      }}>Revert</button>
                    ) : (
                      <span class="list-row-details" style="font-size: var(--font-size-md)">Reverted</span>
                    )}
                  </div>
                </div>
              ))}
              {hasMore && (
                <div
                  ref={sentinelRef}
                  class="dropdown-panel-loading-more"
                  style={loadingMore ? undefined : 'opacity: 0.4'}
                >
                  {loadingMore ? 'Loading more...' : 'Scroll for more'}
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
