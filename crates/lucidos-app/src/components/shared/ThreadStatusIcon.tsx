import type { ThreadStatus } from '../../store/thread-events';

/** 'changes' = static dot (CC has pending changes); 'question' = "?" badge
 *  (CC paused on AskUserQuestion). 'waiting' = pulsing dot, used only when
 *  the thread has no own state of its own to surface (otherwise own state
 *  wins, even with active children). */
export type VisualStatus = ThreadStatus | 'changes' | 'question';

export function resolveVisualStatus(
  status: ThreadStatus,
  hasActiveChildren: boolean,
  ccHasChanges: boolean,
): VisualStatus {
  if (status === 'failed') return 'failed';
  if (status === 'running') return 'running';
  if (status === 'waiting_for_user_answer') return 'question';
  if (ccHasChanges) return 'changes';
  if (hasActiveChildren) return 'waiting';
  return 'idle';
}

interface Props {
  status: VisualStatus | null;  // null = not loaded yet
}

export function ThreadStatusIcon({ status }: Props) {
  if (status === null) return (
    <span class="thread-status thread-status-loading">
      <span class="progress-dot progress-dot-loading" />
    </span>
  );
  return (
    <span class={`thread-status thread-status-${status}`}>
      {status === 'running' && (
        <span class="mini-spinner" />
      )}
      {status === 'waiting' && (
        <span class="progress-dot progress-dot-waiting" />
      )}
      {status === 'changes' && (
        <span class="progress-dot progress-dot-changes" />
      )}
      {status === 'question' && (
        <span class="thread-status-question-badge" aria-label="Waiting for your answer" />
      )}
      {status === 'failed' && (
        <span class="progress-dot progress-dot-failed" aria-label="Last response failed" />
      )}
{/* idle = no icon — it's the default state for every finished thread */}
    </span>
  );
}
