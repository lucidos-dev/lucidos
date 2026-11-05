import type { CleanupRow } from '../../api/client/control';

const STALE_MS = 30 * 24 * 60 * 60 * 1000;
export const CLEANUP_NUDGE_BYTES = 1024 ** 3;
export const CLEANUP_NUDGE_INTERVAL_MS = 7 * 24 * 60 * 60 * 1000;

/** Presentation suggestions never authorize deletion; every choice is frozen
 * and re-observed by the gateway after the user requests review. */
export function isCleanupSuggested(row: CleanupRow, now = Date.now()): boolean {
  const workspace = row.workspace;
  if (!workspace || !['eval', 'e2e', 'demo'].includes(workspace.origin)
    || row.running !== false || !row.fingerprint || row.warning || row.database?.warning) return false;
  if (workspace.ephemeral && workspace.expires_at
    && Date.parse(workspace.expires_at) <= now) return true;
  if (!['eval', 'e2e'].includes(workspace.origin)) return false;
  const dates = [row.folder_modified, row.database?.last_event, workspace.last_opened_at]
    .filter((value): value is string => value != null).map(Date.parse);
  return dates.length > 0 && dates.every(Number.isFinite) && Math.max(...dates) <= now - STALE_MS;
}

export function cleanupTotals(rows: CleanupRow[]): { count: number; knownBytes: number; unknownSizes: number } {
  let knownBytes = 0;
  let unknownSizes = 0;
  for (const row of rows) {
    knownBytes += (row.folder_bytes ?? 0) + (row.database?.bytes ?? 0);
    if ((row.state !== 'database-only' && row.folder_bytes == null)
      || ((row.workspace != null || row.state === 'database-only') && row.database == null)) unknownSizes++;
  }
  return { count: rows.length, knownBytes, unknownSizes };
}

export function sortCleanupRows(rows: CleanupRow[]): CleanupRow[] {
  return [...rows].sort((a, b) => {
    const aTotal = cleanupTotals([a]);
    const bTotal = cleanupTotals([b]);
    return aTotal.unknownSizes - bTotal.unknownSizes || bTotal.knownBytes - aTotal.knownBytes
      || a.id.localeCompare(b.id);
  });
}
