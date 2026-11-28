
import type { ApplyEstimates, Change, LandingLane, MainCheck } from '../../api/client';
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
  landingLane,
  lanePlace,
  mainCheck,
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
import { LANE_PLACE_LABEL } from './landingLane';

export const UNTITLED_THREAD = 'Untitled thread';

export interface ApplyThread {
  threadId: string;
  changeId: string | null;
  reading: ApplyPhaseReading | null;
  title: string;
  phase: string;
}

export type ActivityBody =
  | { kind: 'background'; activity: BackgroundActivity }
  | {
      kind: 'apply-all';
      thread: ApplyThread | null;
      count: { done: number; total: number } | null;
      progress: StepProgress | null;
      timeLeft: string | null;
      canceling: boolean;
    }
  | { kind: 'apply-thread'; thread: ApplyThread }
  | { kind: 'landing-lane' };

export interface ActivityRow {
  key: string;
  label: string;
  detail?: string;
  queued?: boolean;
  body: ActivityBody;
}

export interface ActivityRowsInput {
  activities: readonly BackgroundActivity[];
  applyAllActive: boolean;
  applyAllCanceling: boolean;
  batch: ApplyAllBatch | null;
  pending: readonly Change[] | null;
  lane: LandingLane;
  mainCheck: MainCheck;
  phases: ReadonlyMap<string, ApplyPhaseReading>;
  applyingNow: ReadonlySet<string>;
  estimates: ApplyEstimates;
  nowMs: number;
}

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
    lane: landingLane.value,
    mainCheck: mainCheck.value,
    phases: applyPhases.value,
    applyingNow: applyingNowThreadIds.value,
    estimates: applyEstimates.value,
    nowMs,
  });
}

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
  const laneRow = landingLaneRow(input);
  if (laneRow) rows.push(laneRow);
  rows.push(...singleApplyRows(input));
  return rows;
}

function landingLaneRow({ lane, mainCheck }: ActivityRowsInput): ActivityRow | null {
  const row = { key: 'landing-lane', body: { kind: 'landing-lane' } } as const;
  const waiting = lane.waiting.length;
  if (lane.open_batch) {
    return {
      ...row,
      label: `Landing ${countOf(lane.open_batch.members.length)}`,
      detail: waiting > 0 ? `${waiting} waiting` : undefined,
      queued: false,
    };
  }
  if (waiting > 0) return { ...row, label: `${countOf(waiting)} waiting to land`, queued: true };
  if (mainCheck.state === 'running') return { ...row, label: 'Checking main', queued: false };
  if (mainCheck.state === 'red') return { ...row, label: 'Main is red', queued: true };
  if (lane.multiple_landers) return { ...row, label: 'Multiple landers detected', queued: true };
  return null;
}

function countOf(n: number): string {
  return `${n} change${n === 1 ? '' : 's'}`;
}

function applyAllRow(input: ActivityRowsInput): ActivityRow | null {
  const { applyAllActive, applyAllCanceling, batch, pending, phases, estimates, nowMs } = input;
  if (!applyAllActive) return null;
  const pendingIds = pending ? new Set(pending.map((c) => c.id)) : null;
  const inFlight = batch ? batchMemberInFlight(batch, pendingIds) : null;
  const count = inFlight ? { done: inFlight.resolved, total: inFlight.total } : null;
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
    detail: count ? `${count.done} done` : undefined,
    body: { kind: 'apply-all', thread, count, progress, timeLeft, canceling: applyAllCanceling },
  };
}

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
    if (lanePlace(input.lane, change?.id)) continue;
    const thread = applyThread(threadId, change, input);
    rows.push({ key: `apply-${threadId}`, label: applyLabel(thread), body: { kind: 'apply-thread', thread } });
  }
  return rows;
}

function applyThread(
  threadId: string,
  change: Change | undefined,
  { lane, phases, estimates, nowMs }: ActivityRowsInput,
): ApplyThread {
  const reading = applyPhaseOf(threadId, change, phases);
  const place = lanePlace(lane, change?.id);
  return {
    threadId,
    changeId: change?.id ?? null,
    reading,
    title: threadTitle(threadId, change) ?? UNTITLED_THREAD,
    phase: place ? LANE_PLACE_LABEL[place] : phaseLabel(reading, estimates, nowMs),
  };
}

function applyVerb(reading: ApplyPhaseReading | null): string {
  return reading ? APPLY_PHASE_LABEL[reading.phase] : 'Applying';
}

function applyingCount(total: number): string {
  return `Applying ${total} change${total === 1 ? '' : 's'}`;
}

function applyLabel(thread: ApplyThread): string {
  return `${applyVerb(thread.reading)}: ${thread.title}`;
}
