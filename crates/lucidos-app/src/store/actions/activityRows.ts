/**
 * The *activity group*'s rows: one per job in flight, for the Lucidos menu and
 * the brand badge alike. One derivation feeds both, so the badge cannot spin
 * for a job the menu does not list.
 *
 * Background activities (builds, the model download, an Expose run) come in
 * already derived by `store/backgroundActivity.ts`. Applies are added here:
 * the Apply All batch as one row, and each single-thread apply as its own. A
 * batch member parked on a merge conflict counts as its own apply too.
 * Their rows read the pending changes and the batch, which that module never sees.
 *
 * `activityRows` is pure of its arguments except for the thread titles, which
 * it reads the way the apply toasts do (`threadTitle`). `liveActivityRows`
 * feeds it from the signals.
 */

import type { ApplyEstimates, Change } from '../../api/client';
import {
  applyAllBatch,
  applyAllCanceling,
  applyAllInProgress,
  applyEstimates,
  applyingNowThreadIds,
  applyPhases,
  changes,
  embeddingModelStatus,
  engineBuildDetail,
  engineBuilding,
  frontendRefreshDetail,
  tailscaleServeRun,
  type ApplyAllBatch,
  type ApplyPhaseReading,
} from '../store';
import { backgroundActivities, type BackgroundActivity } from '../backgroundActivity';
import type { StepProgress } from '../types';
import {
  applyPhaseOf,
  APPLY_PHASE_LABEL,
  batchMemberInFlight,
  batchMembersParkedBeside,
  batchSecondsLeft,
  batchTimeLeftLabel,
  phaseLabel,
  threadTitle,
} from './applyProgress';

/** The thread an apply is working on, and the event its phase started at. */
export interface ApplyThread {
  threadId: string;
  changeId: string | null;
  reading: ApplyPhaseReading | null;
  title: string;
  /** What the apply is doing, with times for a slow phase:
   *  "Resolving merge conflict · 6 min, usually ~18 min". */
  phase: string;
}

/** What a row unfolds to in the menu, as data. `ActivityMenuRows.tsx` draws it. */
export type ActivityBody =
  /** A build, the model download or an Expose run. */
  | { kind: 'background'; activity: BackgroundActivity }
  /** The Apply All batch. `thread` and `position` are `null` until the batch
   *  names its member in flight. */
  | {
      kind: 'apply-all';
      thread: ApplyThread | null;
      position: { index: number; total: number } | null;
      progress: StepProgress | null;
      /** "about 20 min until all are applied", or `null` when the batch
       *  cannot say. */
      timeLeft: string | null;
      canceling: boolean;
    }
  /** A single-thread apply. */
  | { kind: 'apply-thread'; thread: ApplyThread };

export interface ActivityRow {
  /** Stable per job, for list keys and for which rows are unfolded. */
  key: string;
  label: string;
  /** Right-aligned beside the label: elapsed time, bytes, "2 of 4". */
  detail?: string;
  /** The job waits its turn instead of running, so nothing should spin. */
  queued?: boolean;
  body: ActivityBody;
}

export interface ActivityRowsInput {
  activities: readonly BackgroundActivity[];
  /** `applyAllInProgress`: true from the click, before the batch is known. */
  applyAllActive: boolean;
  /** `applyAllCanceling`: a Cancel was pressed and the batch has not ended. */
  applyAllCanceling: boolean;
  batch: ApplyAllBatch | null;
  /** The pending changes, or `null` while the list is not loaded. */
  pending: readonly Change[] | null;
  phases: ReadonlyMap<string, ApplyPhaseReading>;
  /** `applyingNowThreadIds`: the threads with an Apply Now in flight. */
  applyingNow: ReadonlyMap<string, unknown>;
  estimates: ApplyEstimates;
  nowMs: number;
}

/** The rows as the signals stand now. Reading them here subscribes the
 *  caller's render, which is how the badge and the menu stay live. */
export function liveActivityRows(nowMs: number = Date.now()): ActivityRow[] {
  const loaded = changes.value;
  return activityRows({
    activities: backgroundActivities(
      engineBuilding.value,
      embeddingModelStatus.value,
      tailscaleServeRun.value,
      engineBuildDetail.value,
      nowMs,
      frontendRefreshDetail.value,
    ),
    applyAllActive: applyAllInProgress.value,
    applyAllCanceling: applyAllCanceling.value,
    batch: applyAllBatch.value,
    pending: loaded.status === 'loaded' ? loaded.data : null,
    phases: applyPhases.value,
    applyingNow: applyingNowThreadIds.value,
    estimates: applyEstimates.value,
    nowMs,
  });
}

/** Every job in flight, builds first, then applies. */
export function activityRows(input: ActivityRowsInput): ActivityRow[] {
  const rows: ActivityRow[] = input.activities.map((activity) => ({
    key: activity.kind,
    label: activity.label,
    detail: activity.detail,
    queued: activity.queued !== undefined,
    body: { kind: 'background', activity },
  }));
  const batchRow = applyAllRow(input);
  if (batchRow) rows.push(batchRow);
  rows.push(...singleApplyRows(input));
  return rows;
}

function applyAllRow(input: ActivityRowsInput): ActivityRow | null {
  const { applyAllActive, applyAllCanceling, batch, pending, phases, estimates, nowMs } = input;
  if (!applyAllActive) return null;
  const pendingIds = pending ? new Set(pending.map((c) => c.id)) : null;
  const inFlight = batch ? batchMemberInFlight(batch, pendingIds) : null;
  const position = inFlight ? { index: inFlight.position, total: inFlight.total } : null;
  // Every member started and not yet done is working: the one applying and
  // the parked ones alike.
  const progress = inFlight
    ? {
      done: inFlight.resolved / inFlight.total,
      working: (inFlight.position - inFlight.resolved) / inFlight.total,
    }
    : null;
  const timeLeft = batch && inFlight
    ? batchTimeLeftLabel(batchSecondsLeft(batch, pending, phases, estimates, nowMs))
    : null;
  const change = inFlight ? pending?.find((c) => c.id === inFlight.changeId) : undefined;
  const thread = change?.thread_id ? applyThread(change.thread_id, change, input) : null;
  const label = applyAllCanceling
    ? 'Canceling apply...'
    : batch ? applyingCount(batch.changeIds.length) : 'Applying changes';
  return {
    key: 'apply-all',
    label,
    detail: position ? `${position.index} of ${position.total}` : undefined,
    body: { kind: 'apply-all', thread, position, progress, timeLeft, canceling: applyAllCanceling },
  };
}

/** One row per thread applying on its own: an Apply Now, or a Changes-panel
 *  apply that is hardening or resolving a conflict. A batch member's phase
 *  belongs to the Apply All row, so it gets none, unless it is parked beside
 *  the member that row names. A reloaded page saw no phase event, so a
 *  conflict also counts when the served row says so. */
function singleApplyRows(input: ActivityRowsInput): ActivityRow[] {
  const { batch, pending, phases, applyingNow } = input;
  const pendingIds = pending ? new Set(pending.map((c) => c.id)) : null;
  const parked = new Set(batch ? batchMembersParkedBeside(batch, pendingIds) : []);
  const batchIds = new Set((batch?.changeIds ?? []).filter((id) => !parked.has(id)));
  const resolving = (pending ?? []).filter((c) => c.resolving_conflict).map((c) => c.thread_id);
  const threadIds = new Set([
    ...applyingNow.keys(),
    ...phases.keys(),
    ...resolving.filter((id): id is string => !!id),
  ]);
  const rows: ActivityRow[] = [];
  for (const threadId of threadIds) {
    const change = pending?.find((c) => c.thread_id === threadId);
    if (change && batchIds.has(change.id)) continue;
    const thread = applyThread(threadId, change, input);
    rows.push({ key: `apply-${threadId}`, label: applyLabel(thread), body: { kind: 'apply-thread', thread } });
  }
  return rows;
}

function applyThread(
  threadId: string,
  change: Change | undefined,
  { phases, estimates, nowMs }: ActivityRowsInput,
): ApplyThread {
  const reading = applyPhaseOf(threadId, change, phases);
  return {
    threadId,
    changeId: change?.id ?? null,
    reading,
    title: threadTitle(threadId, change) ?? 'Untitled thread',
    phase: phaseLabel(reading, estimates, nowMs),
  };
}

/** What the apply is doing, as a verb: "Hardening", or "Applying" when nothing says. */
function applyVerb(reading: ApplyPhaseReading | null): string {
  return reading ? APPLY_PHASE_LABEL[reading.phase] : 'Applying';
}

/** The batch as a whole, so "2 of 5" counts changes, not merge conflicts. */
function applyingCount(total: number): string {
  return `Applying ${total} change${total === 1 ? '' : 's'}`;
}

function applyLabel(thread: ApplyThread): string {
  return `${applyVerb(thread.reading)}: ${thread.title}`;
}
