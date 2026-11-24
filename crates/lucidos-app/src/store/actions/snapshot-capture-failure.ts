import { formatBytes } from '../../utils/formatBytes';

const GENERIC = 'Snapshot capture failed. Review snapshot status before trying again.';

export function snapshotCaptureFailureMessage(reason: unknown): string {
  const needed = (reason as { 'disk-floor'?: { needed_bytes?: unknown } } | null)?.['disk-floor']?.needed_bytes;
  if (typeof needed === 'number') {
    return `Not enough free disk space for a snapshot. Free up ${formatBytes(needed, Math.ceil)} and try again.`;
  }
  return GENERIC;
}
