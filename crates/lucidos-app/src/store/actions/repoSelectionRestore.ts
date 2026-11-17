import { readWindowItem, removeWindowItem } from '../../utils/windowStorage';
import { panelOverlay, parseRepoPath, SELECTED_CHANGE_KEY, showToast } from '../store';
import { getChangeById, ApiError } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { lazyModule } from '../../utils/lazyComponent';
const context = lazyModule(() => import('./repositories'));

/** Restore the user's last-selected change after a reload. Without this, the
 *  Files panel always lands on the All Files tree even if the user was
 *  viewing a Diff. Only definitively-gone IDs (404 from the engine) drop the
 *  saved id; transient failures (network, 5xx) keep it so the next reload
 *  retries: silently nuking on transient errors would lose the user's
 *  selection because of a momentary outage. */
export async function restoreRepoSelectionFromStorage(): Promise<void> {
  const savedId = readWindowItem(SELECTED_CHANGE_KEY);
  if (!savedId) return;
  // If a file-preview overlay re-hydrated for the same change, RepoFilePreview's
  // useEffect already calls loadChangeContextById: skip to avoid a duplicate
  // round-trip for getChangeById/getChangeDiff/listRepoFiles on every reload.
  const overlay = panelOverlay.value;
  if (overlay?.type === 'file-preview') {
    const parsed = parseRepoPath(overlay.path);
    if (parsed?.mode === 'diff' && parsed.changeId === savedId) return;
  }
  try {
    const changeRead = getChangeById(savedId);
    const loaded = context.preload();
    const change = await changeRead;
    if (!await loaded) throw new Error('File context code could not load. Retry or refresh Lucidos.');
    if (readWindowItem(SELECTED_CHANGE_KEY) !== savedId) return;
    await context.get().loadChangeContext(change);
  } catch (e) {
    if (readWindowItem(SELECTED_CHANGE_KEY) !== savedId) return;
    if (e instanceof ApiError && e.httpCode === 404) {
      removeWindowItem(SELECTED_CHANGE_KEY);
    } else showToast(`Could not restore the selected change: ${errorDetail(e)}`, 'error', {
      key: 'repo-selection-restore', action: { label: 'Retry', onClick: () => { void restoreRepoSelectionFromStorage(); } },
    });
  }
}

