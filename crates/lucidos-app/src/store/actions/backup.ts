import { ApiError, createBackup, getLastSuccessfulBackup } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { backupProgress, lastSuccessfulBackup, showToast } from '../store';
import { setLoadingIfFresh, toFailed } from '../types';

/** Queue a backup to `provider`. Completion arrives through SSE
 *  (`BackupCompleted` / `BackupFailed`), which clears `backupProgress`.
 *
 *  The one start path: Settings and the overdue banner both call it, so the
 *  failure toasts have a single copy. A 409 shows the engine's own message:
 *  it may be a running job, missing setup or a provider that is not ready. */
export async function startBackup(provider: string): Promise<void> {
  backupProgress.value = { phase: 'starting', progress: 0, total: 0 };
  try {
    await createBackup(provider);
  } catch (err) {
    backupProgress.value = null;
    if (err instanceof ApiError && err.httpCode === 409) {
      showToast(err.reason, 'warning');
    } else {
      showToast(`Backup failed: ${errorDetail(err)}`, 'error');
    }
  }
}

/** Read the workspace's own backup history for the app-shell banner.
 *
 *  A re-read keeps the loaded answer on screen while it runs. A failed read
 *  replaces it, never keeps it: the banner fails closed, and a stale "overdue"
 *  would outlive the backup that cleared it.
 *
 *  Only the newest read writes. Reads overlap (startup, a gap repair, a
 *  `BackupCompleted`), and an older answer landing last would bring back an
 *  "overdue" the backup just cleared. */
let lastSuccessfulReadSeq = 0;

export async function loadLastSuccessfulBackup(): Promise<void> {
  const seq = ++lastSuccessfulReadSeq;
  setLoadingIfFresh(lastSuccessfulBackup);
  try {
    const data = await getLastSuccessfulBackup();
    if (seq === lastSuccessfulReadSeq) lastSuccessfulBackup.value = { status: 'loaded', data };
  } catch (err) {
    if (seq === lastSuccessfulReadSeq) lastSuccessfulBackup.value = toFailed(err);
  }
}
