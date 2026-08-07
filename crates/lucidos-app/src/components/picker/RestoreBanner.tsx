/**
 * The picker's restore-from-backup status banner: running, completed, failed.
 *
 * A pure vnode builder taking its whole state as props (same shape as
 * `pickerFooter` next door), so what each state renders is unit-testable without
 * a DOM and `WorkspacePicker.tsx` keeps only the wiring.
 *
 * Two rules the states do NOT share:
 *
 * - **Completed carries no buttons and leaves on its own.** By the time it says
 *   "Restored X" the restored workspace is already a row in the list above,
 *   healthy and clickable, so an Open button duplicates the row and a Dismiss
 *   button asks the user to tidy up after a success. The caller clears the
 *   gateway's terminal status on a timer instead (see `WorkspacePicker`).
 * - **Failed keeps its Dismiss.** An error the user has not read must not
 *   disappear on a timer.
 */

import type { VNode } from 'preact';
import type { GwRestoreStatus } from '../../api/client/control';

/** Coarse phase labels the engine `restore-archive` CLI reports through the
 *  gateway's restore status. */
const RESTORE_PHASE_LABELS: Record<string, string> = {
  starting: 'Starting…',
  restoring: 'Restoring…',
  decrypting: 'Decrypting…',
  decompressing: 'Decompressing…',
  initializing: 'Unpacking files…',
  restoring_db: 'Restoring database…',
  done: 'Finishing…',
};

export interface RestoreBannerProps {
  /** Latest gateway restore status, or null before the first poll lands. */
  status: GwRestoreStatus | null;
  busy: boolean;
  /** Acknowledge a FAILED restore. Not offered for the other states. */
  onDismiss: () => void;
}

export function restoreBanner(p: RestoreBannerProps): VNode | null {
  const s = p.status;
  // Null before the first poll lands; idle once nothing is in flight and no
  // result is outstanding. Neither has a banner.
  if (!s || s.status === 'idle') return null;

  if (s.status === 'running') {
    return (
      <div class="ws-picker-restore-banner" data-state="running">
        {/* The spinner must stay a 1rem circle, so the flexing element is
            addressed by class. A positional `> span:first-of-type` looks like it
            names the message, but the spinner is the first span here: it won the
            cascade on specificity and stretched into a banner-wide rotating
            ellipse. */}
        <span class="ws-picker-restore-spinner" />
        <span class="ws-picker-restore-text">
          Restoring “{s.name}”: {RESTORE_PHASE_LABELS[s.phase] || s.phase}
        </span>
      </div>
    );
  }

  if (s.status === 'completed') {
    return (
      <div class="ws-picker-restore-banner" data-state="completed">
        <span class="ws-picker-restore-text">Restored “{s.name}”</span>
      </div>
    );
  }

  return (
    <div class="ws-picker-restore-banner" data-state="failed">
      <span class="ws-picker-restore-text">Restore failed: {s.error}</span>
      <button class="ws-picker-btn" disabled={p.busy} onClick={p.onDismiss}>Dismiss</button>
    </div>
  );
}
