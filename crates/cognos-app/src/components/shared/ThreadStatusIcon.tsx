import type { ThreadStatus } from '../../store/thread-events';

/** 'changes' = static dot (CC has pending changes); 'question' = "?" badge
 *  (CC paused on AskUserQuestion). Active children render the same pulsing
 *  dot as 'waiting'. */
export type VisualStatus = ThreadStatus | 'changes' | 'question';

export function resolveVisualStatus(
  status: ThreadStatus,
  hasActiveChildren: boolean,
  ccHasChanges: boolean,
): VisualStatus {
  if (hasActiveChildren) return 'waiting';
  if (status === 'waiting_for_user_answer') return 'question';
  // Backend shouldn't park threads in 'waiting' without ccHasChanges; treat
  // that combination as 'idle' so legacy rows render without a stale dot.
  if (status === 'waiting') return ccHasChanges ? 'changes' : 'idle';
  return status;
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
