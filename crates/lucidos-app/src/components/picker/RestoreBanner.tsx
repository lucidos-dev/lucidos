
import type { VNode } from 'preact';
import type { GwRestoreStatus } from '../../api/client/control';

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
  status: GwRestoreStatus | null;
  busy: boolean;
  onDismiss: () => void;
}

export function restoreNeedsReading(s: GwRestoreStatus): boolean {
  return s.status === 'completed' && s.warnings.length > 0;
}

export function restoreBanner(p: RestoreBannerProps): VNode | null {
  const s = p.status;
  if (!s || s.status === 'idle') return null;

  if (s.status === 'running') {
    return (
      <div class="ws-picker-restore-banner" data-state="running">
        <span class="mini-spinner ws-picker-restore-spinner" />
        <span class="ws-picker-restore-text">
          Restoring “{s.name}”: {RESTORE_PHASE_LABELS[s.phase] || s.phase}
        </span>
      </div>
    );
  }

  if (s.status === 'completed' && !restoreNeedsReading(s)) {
    return (
      <div class="ws-picker-restore-banner" data-state="completed">
        <span class="ws-picker-restore-text">Restored “{s.name}”</span>
      </div>
    );
  }

  if (s.status === 'completed') {
    return (
      <div class="ws-picker-restore-banner" data-state="completed-with-warnings">
        <span class="ws-picker-restore-text">
          Restored “{s.name}”, but: {s.warnings.join(' ')} The backup still holds them.
        </span>
        <button class="ws-picker-btn" disabled={p.busy} onClick={p.onDismiss}>Dismiss</button>
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
