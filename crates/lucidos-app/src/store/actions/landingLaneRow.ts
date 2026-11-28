
import type { Change, LandingLane, LaneStep, MainCheck } from '../../api/client';
import { changes, landingLane, landingLaneCanceling, mainCheck, mainCheckClosing } from '../store';
import { UNTITLED_THREAD } from './activityRows';
import { threadTitle } from './applyProgress';

const LANE_THREAD_FALLBACK_TITLE = 'Lane thread';
const FIX_THREAD_FALLBACK_TITLE = 'Fix thread';

export const LANE_STEP_LABEL: Record<LaneStep, string> = {
  resolving: 'Resolving merge conflicts',
  landing: 'Landing',
};

export const LANE_GATHERING_LABEL = 'Waiting for Apply All to queue the rest';
export const LANE_WAITING_FOR_ORIGIN_LABEL = 'Waiting for origin';

export interface LaneEntry {
  changeId: string;
  threadId: string | null;
  title: string;
  remote?: true;
}

export interface LaneBatchView {
  batchId: string;
  step: LaneStep;
  laneThread: { threadId: string; title: string };
  members: LaneEntry[];
}

export type MainCheckLine =
  | { kind: 'passing' }
  | { kind: 'running'; runUrl: string | null }
  | {
      kind: 'red';
      repair?: MainCheck['repair'];
      flakyOnly?: boolean;
      checkId: string;
      summary: string;
      fixThread: { threadId: string; title: string } | null;
      closing: boolean;
    };

export interface LaneView {
  batch: LaneBatchView | null;
  waiting: LaneEntry[];
  gathering: boolean;
  canceling: boolean;
  waitingForOrigin?: string;
  waitingForLander?: boolean;
  multipleLanders?: boolean;
  mainCheck: MainCheckLine | null;
}

export interface LaneViewInput {
  lane: LandingLane;
  laneCanceling: string | null;
  pending: readonly Change[] | null;
  mainCheck: MainCheck;
  mainCheckClosing: string | null;
}

function mainCheckLine(check: MainCheck, closing: string | null): MainCheckLine | null {
  if (check.repair && (check.state === 'red' || check.repair.state !== 'none')) {
    const threadId = check.repair.state === 'running' ? check.repair.fix_thread_id : undefined;
    return {
      kind: 'red', repair: check.repair, flakyOnly: check.state !== 'red',
      checkId: check.check_id ?? '',
      summary: check.summary ?? 'flaky tests',
      fixThread: threadId ? { threadId, title: threadTitle(threadId, undefined) ?? FIX_THREAD_FALLBACK_TITLE } : null,
      closing: !!check.check_id && closing === check.check_id,
    };
  }
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

export function landingLaneView({ lane, laneCanceling, pending, mainCheck, mainCheckClosing }: LaneViewInput): LaneView {
  const entry = (changeId: string): LaneEntry => {
    const remote = lane.remote_members?.find((member) => member.change_id === changeId);
    if (remote) {
      const title = remote.gateway ? `${remote.title} · ${remote.gateway}` : remote.title;
      return { changeId, threadId: null, title, remote: true };
    }
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
    ...(lane.setting === 'another-host' ? { waitingForLander: true } : {}),
    ...(lane.waiting_for_origin ? { waitingForOrigin: lane.waiting_for_origin } : {}),
    ...(lane.multiple_landers ? { multipleLanders: true } : {}),
    mainCheck: mainCheckLine(mainCheck, mainCheckClosing),
  };
}

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
