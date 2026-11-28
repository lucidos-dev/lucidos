
import type { ThreadMeta, ThreadStatus, ThreadState } from '../../store/thread-events';
import { changeAwaitsApply, effectiveThreadStatus, readRequestedForMe } from '../../store/store';

export type VisualStatus = ThreadStatus | 'changes' | 'read-request' | 'question';

export function resolveVisualStatus(
  status: ThreadStatus,
  waitsOnSubThreads: boolean,
  changeReady: boolean,
  hasLiveEventWaits: boolean,
  readRequested = false,
): VisualStatus {
  if (status === 'failed') return 'failed';
  if (status === 'running') return 'running';
  if (status === 'waiting_for_user_answer') return 'question';
  if (status === 'paused') return 'paused';
  if (readRequested && !changeReady) return 'read-request';
  if (hasLiveEventWaits) return 'waiting';
  if (changeReady) return 'changes';
  if (waitsOnSubThreads) return 'waiting';
  return 'idle';
}

type VisualStatusFacts = Pick<
  ThreadMeta,
  'id' | 'awaitedChildrenCount' | 'codingAgentChangeState' | 'liveEventWaitCount' | 'readRequested' | 'replySeenDevices'
>;

export function visualStatusFor(status: ThreadStatus, meta: VisualStatusFacts | undefined): VisualStatus {
  if (!meta) return resolveVisualStatus(status, false, false, false);
  return resolveVisualStatus(
    status,
    (meta.awaitedChildrenCount ?? 0) > 0,
    changeAwaitsApply(meta),
    meta.liveEventWaitCount > 0,
    readRequestedForMe(meta),
  );
}

export function threadVisualStatus(thread: ThreadState): VisualStatus {
  return visualStatusFor(effectiveThreadStatus(thread), thread.meta);
}
