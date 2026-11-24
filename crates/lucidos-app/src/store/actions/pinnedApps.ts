import { pinnedApps, showToast } from '../store';
import type { Loadable, PinnedAppEntry } from '../types';
import { loadedOr, toFailed } from '../types';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';
import { getPinnedAppUis, pinAppApi, unpinAppApi } from '../../api/client';
import { getDeviceId } from './devices';
import { errorDetail } from '../../utils/errorDetail';


function setPinnedApps(entries: PinnedAppEntry[]): void {
  pinnedApps.value = { status: 'loaded', data: entries };
  localStorage.setItem('pinned_apps', JSON.stringify(entries));
}

export function hydratePinnedAppsFromStorage(): Loadable<PinnedAppEntry[]> {
  const saved = localStorage.getItem('pinned_apps');
  if (saved == null) return { status: 'loaded', data: [] };
  try {
    const parsed = JSON.parse(saved) as PinnedAppEntry[];
    return { status: 'loaded', data: parsed };
  } catch (e) {
    return toFailed<PinnedAppEntry[]>(e);
  }
}

export function isAppPinned(appId: string): boolean {
  return loadedOr(pinnedApps.value, []).some((e) => e.app_id === appId);
}

export const pinnedAppsSlice: SliceOf<'pinned_apps'> = {
  begin: () => {},
  apply: (res) => {
    const entries: PinnedAppEntry[] = res.entries.map((e: { app_id?: string }) => ({
      app_id: e.app_id ?? '',
    }));
    const seen = new Set<string>();
    const deduped = entries.filter(e => {
      if (seen.has(e.app_id)) return false;
      seen.add(e.app_id);
      return true;
    });
    setPinnedApps(deduped);
  },
  fail: (e) => {
    showToast(`Failed to load pinned apps: ${errorDetail(e)}`, 'error');
    pinnedApps.value = toFailed(e);
  },
};

export function loadPinnedApps(): Promise<void> {
  return loadSlice('pinned_apps', pinnedAppsSlice, () => getPinnedAppUis(getDeviceId()));
}

export async function pinApp(appId: string): Promise<void> {
  if (isAppPinned(appId)) return;
  setPinnedApps([...loadedOr(pinnedApps.value, []), { app_id: appId }]);

  try {
    await pinAppApi(appId, getDeviceId());
  } catch (e) {
    showToast(`Failed to pin app: ${errorDetail(e)}`, 'error');
    setPinnedApps(loadedOr(pinnedApps.value, []).filter((entry) => entry.app_id !== appId));
  }
}

export async function unpinApp(appId: string): Promise<void> {
  setPinnedApps(loadedOr(pinnedApps.value, []).filter((entry) => entry.app_id !== appId));

  try {
    await unpinAppApi(appId, getDeviceId());
  } catch (e) {
    showToast(`Failed to unpin app: ${errorDetail(e)}`, 'error');
    setPinnedApps([...loadedOr(pinnedApps.value, []), { app_id: appId }]);
  }
}

export function togglePinApp(appId: string): void {
  if (isAppPinned(appId)) {
    void unpinApp(appId);
  } else {
    void pinApp(appId);
  }
}

export function removePinnedAppLocal(appId: string): void {
  const current = loadedOr(pinnedApps.value, []);
  if (!current.some(e => e.app_id === appId)) return;
  setPinnedApps(current.filter(e => e.app_id !== appId));
}
