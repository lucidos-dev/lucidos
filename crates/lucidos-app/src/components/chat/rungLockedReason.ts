import type { ThreadStatus } from '../../store/thread-events';

/** Why a started thread's rung cannot change now, or null when it can. A turn
 *  keeps its rung until it ends (ADR 0542): while it works, waits on an
 *  answer, or is paused. The engine's `rung_locked` also counts a live process,
 *  which the client cannot see, so a rare pick at idle can still be refused. */
export function rungLockedReason(status: ThreadStatus | null): string | null {
  if (status === 'running' || status === 'waiting_for_user_answer' || status === 'paused') {
    return 'The model and effort stay fixed while a turn runs. Pick again when it ends';
  }
  return null;
}
