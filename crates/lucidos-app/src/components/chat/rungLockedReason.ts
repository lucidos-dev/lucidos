import type { ThreadStatus } from '../../store/thread-events';

export function rungLockedReason(status: ThreadStatus | null): string | null {
  if (status === 'running' || status === 'waiting_for_user_answer' || status === 'paused') {
    return 'The model and effort stay fixed while a turn runs. Pick again when it ends';
  }
  return null;
}
