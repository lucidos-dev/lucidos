import { useRef, useCallback, useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { changes, appliedChanges, changeIsReady, changesHasMore, changesLoadingMore, busyChangeIds, inFlightChangeIds, applyAllInProgress, applyAllBatch, applyPhases, landingLane, lanePlace, showConfirm, standingApplyThreadIds, collapsedChangesSectionIds, toggleChangesSectionCollapsed, type ApplyAllBatch, type ApplyPhase, type ApplyPhaseReading, type LanePlace } from '../../store/store';
import { applyPhaseOf, isBatchMember } from '../../store/actions/applyProgress';
import { LANE_PLACE_LABEL, LANE_THREAD_TIP, LANE_WAITING_FOR_LANDER_LABEL } from '../../store/actions/landingLane';
import { removeFromLandingLane } from '../../store/actions/laneControls';
import { applySingleChange, discardSingleChange, applyAllChanges, discardAllChanges, revertChange, loadMoreChanges, armStandingApply, disarmStandingApply, armStandingApplies, disarmStandingApplies, refreshChangesState, APPLY_NEW_VERSION_TOOLTIP } from '../../store/actions/chat-changes';
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

function ChangeHeadline({ change }: { change: Change }) {
  const commits = changeCommitList(change);
  return (
    <>
      <span class="title change-description">{changeHeadline(change)}</span>
      {commits.length > 1 && (
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
  markerClass?: string;
  progressLabel: string | null;
  lane: LanePlace | null;
  armed: boolean;
  onOpen: () => void;
  onDiff: () => void;
  onDiscard: () => void;
  onApply: () => void;
  onStanding: () => void;
  onLaneThread: () => void;
  onRemoveFromLane: () => void;
}

export const THREAD_UNSETTLED_TIP =
  'The coding agent has not finished with this thread. It is working, or waiting for something that will wake it, so wait for it to settle';

export function applyBlockedReason(change: Change): string | null {
  if (change.thread_unsettled) return THREAD_UNSETTLED_TIP;
  if (change.file_count === 0) return 'This change has no file changes left — discard it';
  return null;
}

export type ChangeRowAction =
  | { kind: 'standing'; label: string; tooltip: string }
  | { kind: 'discard' }
  | { kind: 'apply'; label: string; tooltip?: string };

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
    {
      kind: 'apply',
      label: change.requires_restart ? 'Apply*' : 'Apply',
      tooltip: change.requires_restart ? APPLY_NEW_VERSION_TOOLTIP : undefined,
    },
  ];
}

export type LocalRowAction = 'apply' | 'discard' | 'revert';

const APPLY_PHASE_PROGRESS: Record<ApplyPhase, string> = {
  merging: 'Applying...',
  hardening: 'Hardening...',
  'resolving-conflict': 'Resolving...',
};

export function rowProgressLabel(
  change: Change,
  local: LocalRowAction | undefined,
  phases: ReadonlyMap<string, ApplyPhaseReading>,
  batch: ApplyAllBatch | null,
): string {
  if (local === 'discard') return 'Discarding...';
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

function ChangeRow({ change, markerClass, progressLabel, lane, armed, onOpen, onDiff, onDiscard, onApply, onStanding, onLaneThread, onRemoveFromLane }: Partial<ChangeRowProps>) {
  const sk = useSkeleton();
  const layout = rowActionLayout(change ? changeRowActions(change, !!armed) : []);
  const clickable = !sk && !!change?.thread_id;
  const runAction = (kind: ChangeRowAction['kind']) => {
    if (kind === 'standing') onStanding?.();
    else if (kind === 'discard') onDiscard?.();
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
              {change.needs_hardening && ' · Not hardened'}
              {change.resolving_conflict
                ? ' · Resolving merge conflicts'
                : change.thread_unsettled && ' · The thread has not finished'}
              {!change.resolving_conflict && !lane && change.predicted_conflict === 'conflict'
                && ' · Likely merge conflict'}
              {lane && ` · ${lane === 'waiting' && landingLane.value.setting === 'another-host'
                ? LANE_WAITING_FOR_LANDER_LABEL : LANE_PLACE_LABEL[lane]}`}
            </>
          )}
        </SkText>
      </div>
      <div class="list-row-actions">
        <SkBlock w="3rem" h="2rem" round>
          <button class="action-btn" onClick={(e) => { e.stopPropagation(); onDiff?.(); }}>Diff</button>
        </SkBlock>
        {lane ? (
          <button
            class={`action-btn${lane === 'waiting' ? ' action-btn-danger' : ''} change-row-primary`}
            data-tooltip={LANE_ROW_TIP[lane]}
            onClick={(e) => {
              e.stopPropagation();
              if (lane === 'waiting') onRemoveFromLane?.();
              else onLaneThread?.();
            }}
          >
            {lane === 'waiting' ? 'Remove' : 'Lane thread'}
          </button>
        ) : progressLabel ? (
          <SkBlock w="4.75rem" h="2rem" round>
            <button class="action-btn action-btn-confirm change-row-primary" disabled>{progressLabel}</button>
          </SkBlock>
        ) : sk ? (
          <div class="change-row-primary"><SkBlock w="100%" h="2rem" round /></div>
        ) : layout.kind === 'split' ? (
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

const LANE_ROW_TIP: Record<LanePlace, string> = {
  'in-batch': LANE_THREAD_TIP,
  waiting: 'Take this change out of the landing lane. It stays pending, and Apply queues it again.',
};

function rowClass(clickable: boolean, markerClass: string | undefined): string {
  return ['list-row change-row', clickable && 'clickable', markerClass].filter(Boolean).join(' ');
}

export function rowActionClass(action: ChangeRowAction, armed: boolean): string {
  switch (action.kind) {
    case 'discard': return 'action-btn action-btn-danger';
    case 'standing': return armed ? 'action-btn' : 'action-btn action-btn-confirm';
    case 'apply': return 'action-btn action-btn-confirm';
  }
}

function rowActionTooltip(action: ChangeRowAction): string | undefined {
  switch (action.kind) {
    case 'discard': return undefined;
    case 'standing':
    case 'apply': return action.tooltip;
  }
}

function rowActionLabel(action: ChangeRowAction): string {
  switch (action.kind) {
    case 'discard': return 'Discard';
    case 'standing':
    case 'apply': return action.label;
  }
}

export const SETTLE_ALL_TIP =
  'Apply each change here the moment its thread finishes.';

export const SETTLE_ALL_ARMED_TIP =
  'Armed. Each change here applies the moment its thread finishes. Click to cancel. Anything already applying keeps going.';

export interface PendingSections {
  applying: Change[];
  ready: Change[];
  notFinished: Change[];
  canApplyNow: boolean;
  showApplyAll: boolean;
  showDiscardAll: boolean;
  armable: Change[];
  armed: boolean;
}

export function pendingSections(
  pending: Change[],
  armedThreadIds: ReadonlySet<string>,
  applyAllRunning = false,
  inFlightIds: ReadonlySet<string> = new Set(),
): PendingSections {
  const applying = pending.filter((c) => inFlightIds.has(c.id));
  const waiting = pending.filter((c) => !inFlightIds.has(c.id));
  const ready = waiting.filter(changeIsReady);
  const notFinished = waiting.filter((c) => !changeIsReady(c));
  const canApplyNow = ready.some((c) => !applyBlockedReason(c));
  const armable = notFinished.filter((c) => c.thread_settling && !c.resolving_conflict && c.thread_id);
  return {
    applying,
    ready,
    notFinished,
    canApplyNow,
    showApplyAll: canApplyNow || applyAllRunning,
    showDiscardAll: ready.length > 1,
    armable,
    armed: armable.length > 0 && armable.every((c) => armedThreadIds.has(c.thread_id!)),
  };
}

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
  const hasMore = changesHasMore.value;
  const loadingMore = changesLoadingMore.value;
  const showLoadingMore = useDelayedFlag(loadingMore);
  const applyingCollapsed = collapsedChangesSectionIds.value.has('applying');
  const readyCollapsed = collapsedChangesSectionIds.value.has('ready');
  const notFinishedCollapsed = collapsedChangesSectionIds.value.has('not-finished');
  const appliedCollapsed = collapsedChangesSectionIds.value.has('applied');

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

  const showLoading = useDelayedLoading(pendingLoadable);

  const allLoaded =
    pendingLoadable.status === 'loaded' &&
    appliedLoadable.status === 'loaded';
  const armedThreadIds = standingApplyThreadIds.value;
  const sections = pendingLoadable.status === 'loaded'
    ? pendingSections(pendingLoadable.data, armedThreadIds, applyAllInProgress.value, inFlightChangeIds.value)
    : null;
  const ids = (list: Change[] | undefined) => (allLoaded && list ? list.map(c => c.id) : null);
  const applyingArrived = useArrivals(ids(sections?.applying));
  const readyArrived = useArrivals(ids(sections?.ready));
  const pendingArrived = useArrivals(ids(pendingLoadable.status === 'loaded' ? pendingLoadable.data : undefined));

  if (pendingLoadable.status === 'failed' || appliedLoadable.status === 'failed') {
    const err = pendingLoadable.status === 'failed' ? pendingLoadable.error
              : appliedLoadable.status === 'failed' ? appliedLoadable.error
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
          const renderRow = (change: Change, markerClass: string | undefined) => {
            const local = busyIds.value.get(change.id);
            const busy = !!local || busyChangeIds.value.has(change.id) || !!change.resolving_conflict;
            const progressLabel = busy
              ? rowProgressLabel(change, local, applyPhases.value, applyAllBatch.value)
              : null;
            const armed = !!change.thread_id && armedThreadIds.has(change.thread_id);
            const lane = landingLane.value;
            const laneThreadId = lane.open_batch?.lane_thread_id;
            return (
              <ChangeRow
                change={change}
                markerClass={markerClass}
                progressLabel={progressLabel}
                lane={lanePlace(lane, change.id)}
                armed={armed}
                onOpen={() => openChangeThread(change)}
                onDiff={() => void viewChangeDiff(change)}
                onDiscard={() => guardedAction(change.id, 'discard', discardSingleChange)}
                onApply={() => guardedAction(change.id, 'apply', applySingleChange)}
                onStanding={() => {
                  if (!change.thread_id) return;
                  void (armed
                    ? disarmStandingApply(change.thread_id)
                    : armStandingApply(change.thread_id, change.id));
                }}
                onLaneThread={() => { if (laneThreadId) focusThreadOrBootstrap(laneThreadId); }}
                onRemoveFromLane={() => void removeFromLandingLane(change.id)}
              />
            );
          };
          return pending.length === 0 && applied.length === 0 ? (
            <div class="empty-state">No changes</div>
          ) : (
            <>
          <Disclosure open={sections.applying.length > 0}>
            <SectionHeader
              title="Applying"
              count={sections.applying.length}
              collapsed={applyingCollapsed}
              onToggle={() => toggleChangesSectionCollapsed('applying')}
            />
            <Disclosure open={!applyingCollapsed}>
              <ArrivalList items={sections.applying} keyOf={(c) => c.id} arrived={applyingArrived}>
                {renderRow}
              </ArrivalList>
            </Disclosure>
          </Disclosure>
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
