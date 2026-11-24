import { connectionStatus, removeToast, showToast, THREAD_LIST_REFRESH_TOAST_KEY } from '../store';
import { isTransientFetchError } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { loadAllThreads } from './thread-loading';

export async function refreshThreadList(): Promise<void> {
  let landed: boolean;
  try {
    landed = await loadAllThreads();
  } catch (err) {
    reportThreadListRefreshFailure(err);
    return;
  }
  if (!landed) return;
  removeToast(THREAD_LIST_REFRESH_TOAST_KEY);
}

function reportThreadListRefreshFailure(err: unknown): void {
  if (isTransientFetchError(err)) {
    console.warn('[ThreadList] refresh failed transiently; the next sync point re-syncs', err);
    return;
  }
  if (connectionStatus.value !== 'connected') return;
  showToast(`Failed to refresh the thread list: ${errorDetail(err)}`, 'error', {
    key: THREAD_LIST_REFRESH_TOAST_KEY,
  });
}
