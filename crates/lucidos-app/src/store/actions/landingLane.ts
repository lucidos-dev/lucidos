/**
 * The *landing lane* on the client (ADR 0463): what a change row says about
 * its place in it, and what the lane frames do to the `landingLane` signal.
 * The store owns `lanePlace`, since the Review view reads it too.
 *
 * The engine owns the lane. `GET /api/v1/changes` and every `ChangesUpdated`
 * frame carry its snapshot whole. The `LaneBatch*` frames open and close the
 * batch between those, so a batch shows the moment it forms.
 *
 * The first paint loads this module. The lane's presses live in
 * `laneControls.ts` and its menu body in `landingLaneRow.ts`, which only
 * lazy surfaces load (ADR 0454).
 */

import type { LandingLane } from '../../api/client';
import { EMPTY_LANDING_LANE, landingLane, landingLaneCanceling, lanePlace, showToast, type LanePlace } from '../store';
import { focusThreadOrBootstrap } from './threads';

/** What a change row says about its place in the lane. */
export const LANE_PLACE_LABEL: Record<LanePlace, string> = {
  'in-batch': 'Landing in a batch',
  waiting: 'Waiting to land',
};

/** The tooltip on a control that opens the open batch's lane thread. */
export const LANE_THREAD_TIP = 'Open the lane thread that resolves this batch';

/** The lane after `LaneBatchStarted`: the batch opens, and its members leave
 *  the waiting list. It resolves first, since its lane thread has yet to run. */
export function laneWithBatchStarted(
  lane: LandingLane,
  batch: { batch_id: string; lane_thread_id: string; members: readonly { change_id: string }[] },
): LandingLane {
  const members = batch.members.map((m) => m.change_id);
  return {
    open_batch: { batch_id: batch.batch_id, lane_thread_id: batch.lane_thread_id, members, step: 'resolving' },
    waiting: lane.waiting.filter((id) => !members.includes(id)),
    gathering: false,
  };
}

/** The lane after `ChangeQueued` for `changeId`: it waits for the next batch,
 *  unless the lane already places it. The next snapshot replaces this. */
export function laneWithChangeQueued(lane: LandingLane, changeId: string): LandingLane {
  return lanePlace(lane, changeId) ? lane : { ...lane, waiting: [...lane.waiting, changeId] };
}

/** The lane after `changeId` landed or left it. A batch keeps its members,
 *  since its own frame closes it. */
export function laneWithoutWaiting(lane: LandingLane, changeId: string): LandingLane {
  return lane.waiting.includes(changeId)
    ? { ...lane, waiting: lane.waiting.filter((id) => id !== changeId) }
    : lane;
}

/** The lane after `LaneBatchLanded` or `LaneBatchFailed` for `batchId`. */
export function laneWithBatchEnded(lane: LandingLane, batchId: string): LandingLane {
  return lane.open_batch?.batch_id === batchId ? { ...lane, open_batch: null } : lane;
}

/** Take the engine's snapshot. A Cancel pressed for a batch that has since
 *  closed stops reading as canceling. An older engine sends none. */
export function setLandingLaneFromServer(lane: LandingLane | undefined): void {
  const next = lane ?? EMPTY_LANDING_LANE;
  landingLane.value = next;
  if (landingLaneCanceling.value && next.open_batch?.batch_id !== landingLaneCanceling.value) {
    landingLaneCanceling.value = null;
  }
}

/** Apply a lane frame. `LaneBatchStarted` opens the batch; the other two
 *  close it. A failure nobody asked for says why, since its changes go back
 *  to Apply. A Cancel names its actor, and the owner needs no news of it. */
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
