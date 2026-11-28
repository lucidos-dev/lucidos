
import type { LandingLane } from '../../api/client';
import { EMPTY_LANDING_LANE, landingLane, landingLaneCanceling, lanePlace, showToast, type LanePlace } from '../store';
import { focusThreadOrBootstrap } from './threads';

export const LANE_PLACE_LABEL: Record<LanePlace, string> = {
  'in-batch': 'Landing in a batch',
  waiting: 'Waiting to land',
};

export const LANE_WAITING_FOR_LANDER_LABEL = 'Waiting for the lander';

export const LANE_THREAD_TIP = 'Open the lane thread that resolves this batch';

export function laneWithBatchStarted(
  lane: LandingLane,
  batch: { batch_id: string; lane_thread_id: string; members: readonly { change_id: string }[] },
): LandingLane {
  const members = batch.members.map((m) => m.change_id);
  return {
    ...(lane.setting === undefined ? {} : { setting: lane.setting }),
    open_batch: { batch_id: batch.batch_id, lane_thread_id: batch.lane_thread_id, members, step: 'resolving' },
    waiting: lane.waiting.filter((id) => !members.includes(id)),
    gathering: false,
  };
}

export function laneWithChangeQueued(lane: LandingLane, changeId: string): LandingLane {
  return lanePlace(lane, changeId) ? lane : { ...lane, waiting: [...lane.waiting, changeId] };
}

export function laneWithoutWaiting(lane: LandingLane, changeId: string): LandingLane {
  return lane.waiting.includes(changeId)
    ? { ...lane, waiting: lane.waiting.filter((id) => id !== changeId) }
    : lane;
}

export function laneWithBatchEnded(lane: LandingLane, batchId: string): LandingLane {
  return lane.open_batch?.batch_id === batchId ? { ...lane, open_batch: null } : lane;
}

export function setLandingLaneFromServer(lane: LandingLane | undefined): void {
  const next = lane ?? EMPTY_LANDING_LANE;
  landingLane.value = next;
  if (landingLaneCanceling.value && next.open_batch?.batch_id !== landingLaneCanceling.value) {
    landingLaneCanceling.value = null;
  }
}

export function handleLaneFrame(
  type: 'LaneBatchStarted' | 'LaneBatchLanded' | 'LaneBatchFailed',
  data: Record<string, unknown>,
): void {
  const batchId = String(data.batch_id ?? '');
  if (type === 'LaneBatchStarted') {
    landingLane.value = laneWithBatchStarted(landingLane.value, {
      batch_id: batchId,
      lane_thread_id: String(data.lane_thread_id ?? ''),
      members: (data.members ?? []) as { change_id: string }[],
    });
    return;
  }
  const laneThreadId = landingLane.value.open_batch?.batch_id === batchId
    ? landingLane.value.open_batch.lane_thread_id
    : null;
  landingLane.value = laneWithBatchEnded(landingLane.value, batchId);
  if (landingLaneCanceling.value === batchId) landingLaneCanceling.value = null;
  if (type === 'LaneBatchFailed' && !data.actor) {
    showToast(`${String(data.reason ?? 'The lane batch failed.')} Its changes are still pending.`, 'error', {
      title: 'Lane batch failed',
      onClick: laneThreadId ? () => focusThreadOrBootstrap(laneThreadId) : undefined,
    });
  }
}
