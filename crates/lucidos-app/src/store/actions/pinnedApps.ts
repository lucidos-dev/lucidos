import { pinnedApps, showToast } from '../store';
import type { PinnedAppEntry } from '../types';
import { getPinnedAppUis, pinAppApi, unpinAppApi } from '../../api/client';
import { getDeviceId } from './devices';
import { errorDetail } from '../../utils/errorDetail';

function persistToLocalStorage(entries: PinnedAppEntry[]): void {
  localStorage.setItem('pinned_apps', JSON.stringify(entries));
}

/** Check if an app is pinned */
export function isAppPinned(appId: string): boolean {
  return pinnedApps.value.some((e) => e.app_id === appId);
}

/** Load pinned apps from the API and update the signal + localStorage */
export async function loadPinnedApps(): Promise<void> {
  try {
    const res = await getPinnedAppUis(getDeviceId());
    // Map from API response — use app_id, ignore ui_id (legacy)
    const entries: PinnedAppEntry[] = res.entries.map((e: { app_id?: string }) => ({
      app_id: e.app_id ?? '',
    }));
    // Deduplicate by app_id
    const seen = new Set<string>();
    const deduped = entries.filter(e => {
      if (seen.has(e.app_id)) return false;
      seen.add(e.app_id);
      return true;
    });
    pinnedApps.value = deduped;
    persistToLocalStorage(deduped);
  } catch (e) {
    console.error('[PinnedApps] Failed to load pinned apps:', e);
    showToast(`Failed to load pinned apps: ${errorDetail(e)}`, 'error');
  }
}

/** Pin an app — optimistic update, then persist to API */
export async function pinApp(appId: string): Promise<void> {
  if (isAppPinned(appId)) return;
  const updated = [...pinnedApps.value, { app_id: appId }];
  pinnedApps.value = updated;
  persistToLocalStorage(updated);

  try {
    await pinAppApi(appId, getDeviceId());
  } catch (e) {
    console.error('[PinnedApps] Failed to pin app:', e);
    showToast(`Failed to pin app: ${errorDetail(e)}`, 'error');
    // Revert
    const reverted = pinnedApps.value.filter((entry) => entry.app_id !== appId);
    pinnedApps.value = reverted;
    persistToLocalStorage(reverted);
  }
}

/** Unpin an app — optimistic update, then persist to API */
export async function unpinApp(appId: string): Promise<void> {
  const updated = pinnedApps.value.filter((entry) => entry.app_id !== appId);
  pinnedApps.value = updated;
  persistToLocalStorage(updated);

  try {
    await unpinAppApi(appId, getDeviceId());
  } catch (e) {
    console.error('[PinnedApps] Failed to unpin app:', e);
    showToast(`Failed to unpin app: ${errorDetail(e)}`, 'error');
    // Revert
    const reverted = [...pinnedApps.value, { app_id: appId }];
    pinnedApps.value = reverted;
    persistToLocalStorage(reverted);
  }
}

/** Toggle pin state for an app */
export function togglePinApp(appId: string): void {
  if (isAppPinned(appId)) {
    unpinApp(appId);
  } else {
    pinApp(appId);
  }
}

