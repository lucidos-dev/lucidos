/**
 * What the *landing lane*'s row unfolds to in the Lucidos menu: the open
 * batch's step and members, what waits, and the *main check*'s line.
 *
 * `activityRows.ts` gives the row only its label, which is all the header
 * mark reads. The menu builds the body here when it opens, so the first paint
 * never loads it (ADR 0454). `landingLaneView` is pure of its arguments
 * except for the thread titles; `liveLandingLaneView` feeds it the signals.
 */

import type { Change, LandingLane, LaneStep, MainCheck } from '../../api/client';
import { changes, landingLane, landingLaneCanceling, mainCheck, mainCheckClosing } from '../store';
import { UNTITLED_THREAD } from './activityRows';
import { threadTitle } from './applyProgress';

/** The lane thread's title before its first turn names it. */
const LANE_THREAD_FALLBACK_TITLE = 'Lane thread';
/** The fix thread's title before its first turn names it. */
const FIX_THREAD_FALLBACK_TITLE = 'Fix thread';

/** What the open batch is doing, as its lane row says it. */
export const LANE_STEP_LABEL: Record<LaneStep, string> = {
  resolving: 'Resolving merge conflicts',
  landing: 'Landing',
};

/** What the lane row says while it holds conflicts back for an Apply All
 *  press still handing changes over (ADR 0480). */
export const LANE_GATHERING_LABEL = 'Waiting for Apply All to queue the rest';
/** The lead of the line saying origin holds the lane. The reason follows. */
export const LANE_WAITING_FOR_ORIGIN_LABEL = 'Waiting for origin';

/** A change in the landing lane, as its row lists it. */
export interface LaneEntry {
  changeId: string;
  threadId: string | null;
  title: string;
}

/** The open lane batch, as its row shows it. */
export interface LaneBatchView {
  batchId: string;
  step: LaneStep;
  laneThread: { threadId: string; title: string };
  members: LaneEntry[];
}

/** The *main check*'s line in the lane row. Idle and unavailable draw none. */
export type MainCheckLine =
  | { kind: 'passing' }
  | { kind: 'running'; runUrl: string | null }
  | {
      kind: 'red';
      checkId: string;
      summary: string;
      /** The thread that owns the red, or `null` on an engine that names none. */
      fixThread: { threadId: string; title: string } | null;
      /** Close was pressed and the red has not closed yet. */
      closing: boolean;
    };

/** The lane row's body: its open batch, and what waits for the next one. */
export interface LaneView {
  batch: LaneBatchView | null;
  waiting: LaneEntry[];
  /** What waits, waits for an Apply All press, with no batch open. */
  gathering: boolean;
  /** The batch's Cancel was pressed and it has not closed yet. */
  canceling: boolean;
  /** Why origin could not take the last landing, while it holds the lane. */
  waitingForOrigin?: string;
  mainCheck: MainCheckLine | null;
}

export interface LaneViewInput {
  lane: LandingLane;
  /** `landingLaneCanceling`: the batch whose Cancel was pressed. */
  laneCanceling: string | null;
  /** The pending changes, or `null` while the list is not loaded. */
  pending: readonly Change[] | null;
  mainCheck: MainCheck;
  /** `mainCheckClosing`: the red check whose Close was pressed. */
  mainCheckClosing: string | null;
}

/** The main check as its line in the lane row says it. */
function mainCheckLine(check: MainCheck, closing: string | null): MainCheckLine | null {
  switch (check.state) {
    case 'passed':
      return { kind: 'passing' };
    case 'running':
      return { kind: 'running', runUrl: check.run_url ?? null };
    case 'red': {
      const threadId = check.fix_thread_id;
      return {
        kind: 'red',
        checkId: check.check_id ?? '',
        summary: check.summary ?? 'the main check failed',
        fixThread: threadId
          ? { threadId, title: threadTitle(threadId, undefined) ?? FIX_THREAD_FALLBACK_TITLE }
          : null,
        closing: !!check.check_id && closing === check.check_id,
      };
    }
    default:
      return null;
  }
}

/** The lane row's body, as the lane and the main check stand. */
export function landingLaneView({ lane, laneCanceling, pending, mainCheck, mainCheckClosing }: LaneViewInput): LaneView {
  const entry = (changeId: string): LaneEntry => {
    const change = pending?.find((c) => c.id === changeId);
    const threadId = change?.thread_id ?? null;
    return { changeId, threadId, title: threadTitle(threadId, change) ?? UNTITLED_THREAD };
  };
  const open = lane.open_batch;
  return {
    batch: open
      ? {
        batchId: open.batch_id,
        step: open.step,
        laneThread: {
          threadId: open.lane_thread_id,
          title: threadTitle(open.lane_thread_id, undefined) ?? LANE_THREAD_FALLBACK_TITLE,
        },
        members: open.members.map(entry),
      }
      : null,
    waiting: lane.waiting.map(entry),
    gathering: lane.gathering,
    canceling: !!open && laneCanceling === open.batch_id,
    ...(lane.waiting_for_origin ? { waitingForOrigin: lane.waiting_for_origin } : {}),
    mainCheck: mainCheckLine(mainCheck, mainCheckClosing),
  };
}

/** `landingLaneView` from the signals. */
export function liveLandingLaneView(): LaneView {
  const loaded = changes.value;
  return landingLaneView({
    lane: landingLane.value,
    laneCanceling: landingLaneCanceling.value,
    pending: loaded.status === 'loaded' ? loaded.data : null,
    mainCheck: mainCheck.value,
    mainCheckClosing: mainCheckClosing.value,
  });
}
