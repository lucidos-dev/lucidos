import { pinnedApps, showToast } from '../store';
import type { Loadable, PinnedAppEntry } from '../types';
import { loadedOr, toFailed } from '../types';
import { getPinnedAppUis, pinAppApi, unpinAppApi } from '../../api/client';
import { getDeviceId } from './devices';
import { errorDetail } from '../../utils/errorDetail';

// Inlined as a literal (not a `const STORAGE_KEY = ...`) because
// `store.ts` imports `hydratePinnedAppsFromStorage` to initialize the signal,
// and the const would be in TDZ when store.ts re-enters this module via the
// cycle store.ts ↔ pinnedApps.ts. Function declarations hoist; `const` does
// not. Two call sites in this file — duplication is acceptable.

/** Single writer for the signal + localStorage cache. */
function setPinnedApps(entries: PinnedAppEntry[]): void {
  pinnedApps.value = { status: 'loaded', data: entries };
  localStorage.setItem('pinned_apps', JSON.stringify(entries));
}

/** Hydrate from the offline cache. JSON parse failure surfaces as `failed`
 *  instead of silently collapsing to `[]`. */
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

/** Check if an app is pinned */
export function isAppPinned(appId: string): boolean {
  return loadedOr(pinnedApps.value, []).some((e) => e.app_id === appId);
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
    setPinnedApps(deduped);
  } catch (e) {
    showToast(`Failed to load pinned apps: ${errorDetail(e)}`, 'error');
    pinnedApps.value = toFailed(e);
  }
}

/** Pin an app — optimistic update, then persist to API */
export async function pinApp(appId: string): Promise<void> {
  if (isAppPinned(appId)) return;
  setPinnedApps([...loadedOr(pinnedApps.value, []), { app_id: appId }]);

  try {
    await pinAppApi(appId, getDeviceId());
  } catch (e) {
    showToast(`Failed to pin app: ${errorDetail(e)}`, 'error');
    // Revert from current signal state, not a pre-await snapshot — an
    // interleaved AppDeleted SSE may have removed an unrelated entry while
    // the API was in flight, and reverting from a stale snapshot would
    // resurrect it.
    setPinnedApps(loadedOr(pinnedApps.value, []).filter((entry) => entry.app_id !== appId));
  }
}

/** Unpin an app — optimistic update, then persist to API */
export async function unpinApp(appId: string): Promise<void> {
  setPinnedApps(loadedOr(pinnedApps.value, []).filter((entry) => entry.app_id !== appId));

  try {
    await unpinAppApi(appId, getDeviceId());
  } catch (e) {
    showToast(`Failed to unpin app: ${errorDetail(e)}`, 'error');
    setPinnedApps([...loadedOr(pinnedApps.value, []), { app_id: appId }]);
  }
}

/** Toggle pin state for an app. `pinApp` / `unpinApp` toast on failure
 *  themselves, so the caller doesn't need to await. */
export function togglePinApp(appId: string): void {
  if (isAppPinned(appId)) {
    void unpinApp(appId);
  } else {
    void pinApp(appId);
  }
}

/** Local-only prune: drop the entry from the signal + localStorage, no API
 *  call. Used by SSE reconciliation when an app is deleted server-side. */
export function removePinnedAppLocal(appId: string): void {
  const current = loadedOr(pinnedApps.value, []);
  if (!current.some(e => e.app_id === appId)) return;
  setPinnedApps(current.filter(e => e.app_id !== appId));
}
