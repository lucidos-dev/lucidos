import { ApiError, createBackup, getLastSuccessfulBackup } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { backupProgress, lastSuccessfulBackup, showToast } from '../store';
import { setLoadingIfFresh, toFailed } from '../types';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';

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

let lastSuccessfulReadSeq = 0;

export const lastSuccessfulBackupSlice: SliceOf<'last_successful_backup', number> = {
  begin: () => {
    const seq = ++lastSuccessfulReadSeq;
    setLoadingIfFresh(lastSuccessfulBackup);
    return seq;
  },
  apply: (data, seq) => {
    if (seq === lastSuccessfulReadSeq) lastSuccessfulBackup.value = { status: 'loaded', data };
  },
  fail: (err, seq) => {
    if (seq === lastSuccessfulReadSeq) lastSuccessfulBackup.value = toFailed(err);
  },
};

export function loadLastSuccessfulBackup(): Promise<void> {
  return loadSlice('last_successful_backup', lastSuccessfulBackupSlice, () => getLastSuccessfulBackup());
}
