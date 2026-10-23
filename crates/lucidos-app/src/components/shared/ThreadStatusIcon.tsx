import type { VisualStatus } from './threadVisualStatus';
import { DiffIcon, PauseIcon } from './icons';

/** User-facing label + one-line explanation for each status dot, shown in its
 *  hover tooltip. Single source of truth so the dot and its explanation can
 *  never drift. (idle has no dot, so it needs no entry.) */
const STATUS_INFO: { status: VisualStatus; label: string; desc: string }[] = [
  { status: 'running', label: 'Running', desc: 'Actively working on a response.' },
  // One description for both causes, because one dot covers both (see
  // `resolveVisualStatus`). Naming only children would be a lie on a thread
  // that is watching for an event, and the per-wait detail is one tap away in
  // the waiting indicator. A thread with a change and only children reads
  // `changes` instead, so this dot never sits on an applicable change.
  { status: 'waiting', label: 'Waiting', desc: 'Not finished: it is waiting for a child thread, or for an event it subscribed to. While it waits on an event, any proposed change waits with it.' },
  { status: 'question', label: 'Waiting for your answer', desc: 'Paused until you answer its question.' },
  { status: 'changes', label: 'Changes to review', desc: 'A coding agent proposed changes to open and Apply.' },
  { status: 'read-request', label: 'Asked you to read', desc: 'Its agent asked you to read the latest reply. This clears once you have seen it.' },
  { status: 'paused', label: 'Paused', desc: 'Your switch to a new version interrupted this turn. It resumes on its own, so there is nothing to do.' },
  { status: 'failed', label: 'Failed', desc: 'The last response failed.' },
];

/** Tooltip for a status dot: its status name as the bold title, the one-line
 *  explanation as the body — consumed by the global tooltip system
 *  (`data-tooltip-title` / `data-tooltip`). */
export function statusTooltip(status: VisualStatus): { title: string; text: string } {
  const current = STATUS_INFO.find((s) => s.status === status);
  if (!current) return { title: '', text: '' };
  return { title: current.label, text: current.desc };
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
  // idle has no dot to hover, so skip the tooltip (it'd attach to a 0-size span).
  const tip = status === 'idle' ? null : statusTooltip(status);
  return (
    <span
      class={`thread-status thread-status-${status}`}
      data-tooltip={tip?.text}
      data-tooltip-title={tip?.title}
    >
      {status === 'running' && (
        <span class="mini-spinner" />
      )}
      {status === 'waiting' && (
        <span class="progress-dot progress-dot-waiting" />
      )}
      {status === 'changes' && (
        <span class="thread-status-changes-icon" aria-label="Changes to review">
          <DiffIcon />
        </span>
      )}
      {status === 'read-request' && (
        <span class="progress-dot progress-dot-read-request" aria-label="Asked you to read" />
      )}
      {status === 'question' && (
        <span class="thread-status-question-badge" aria-label="Waiting for your answer" />
      )}
      {status === 'paused' && (
        <span class="thread-status-paused-icon" aria-label="Paused by your version switch, resuming on its own">
          <PauseIcon />
        </span>
      )}
      {status === 'failed' && (
        <span class="progress-dot progress-dot-failed" aria-label="Last response failed" />
      )}
{/* idle = no icon — it's the default state for every finished thread */}
    </span>
  );
}
