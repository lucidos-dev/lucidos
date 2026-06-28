import { updateAvailable, showToast, hasRefreshToast } from '../store';
import {
  getServedBuildId,
  refreshClient,
  markSwUpdateDismissed,
  wasSwUpdateDismissed,
  noteUpdateBuildId,
} from '../../hooks/sw-update';
import { CLIENT_BUILD_ID } from 'virtual:build-id';

/** Surface the "New version available → Refresh" toast for a genuinely-served
 *  newer build.
 *
 *  This is the SINGLE place the toast is shown. It is driven by the same honest
 *  build-id check that lights the badge (`syncClientUpdateFromBuild`), so the
 *  toast and the badge can never disagree — the bug where the badge showed but
 *  the toast didn't (because the toast used to hang off the fragile SW
 *  `updatefound` → `activated` event, which is missed when the new worker already
 *  activated before the page attached its listener, or while the page was
 *  backgrounded). Surfacing it from the build-id check is ALSO the moment a
 *  refresh is safe: the served `/sw.js` differs from the loaded bundle, so a
 *  reload genuinely lands on a newer build (not the old one mid-rebuild).
 *
 *  Two guards mirror the prior dedup contract:
 *   - `wasSwUpdateDismissed(served)` — the user already dismissed THIS exact
 *     served build; a genuinely newer build re-surfaces it. The badge is left to
 *     the caller (it stays lit — the update IS available — even when the toast is
 *     suppressed).
 *   - `hasRefreshToast()` — a refresh/restart toast already on screen is itself a
 *     way to act, so don't stack a redundant prompt. Today that's the pre-restart
 *     "Engine restart required" toast (its Restart button) or a live copy of this
 *     toast. The post-restart "Engine restarted" confirmation deliberately
 *     carries NO action, so it does NOT suppress this prompt — a restart that
 *     also rebuilt the client must still surface the refresh.
 *
 *  (`showToast` additionally no-ops while the engine is restarting, so this can't
 *  stack on the "Restarting engine…" status either.) */
export function surfaceUpdateToast(servedBuildId: string): void {
  // Record which build the toast offers so a key-only dismiss (the Toast close
  // button → dismissToast('update-available')) can pin the right id.
  noteUpdateBuildId(servedBuildId);
  if (wasSwUpdateDismissed(servedBuildId)) return;
  if (hasRefreshToast()) return;
  showToast('New version available — refresh to sync', 'info', {
    key: 'update-available',
    action: {
      label: 'Refresh',
      onClick: () => {
        markSwUpdateDismissed();
        // SW-aware: a bare reload keeps the current service worker, so it won't
        // pick up the new sw.js. refreshClient swaps to the new worker (or busts
        // the shell cache if it won't), so the badge clears on the next load.
        refreshClient();
      },
    },
  });
}

/** Sync the "client update available" badge AND the refresh toast to whether the
 *  RUNNING code is older than the build the server is serving — `CLIENT_BUILD_ID`
 *  (the build that produced the code executing right now, stamped into the bundle
 *  at build time) vs the served `/sw.js` BUILD_ID (`getServedBuildId`).
 *
 *  Comparing the LOADED bundle (not the controlling service worker's id) is the
 *  honest signal: it stays lit exactly while the running code is stale and
 *  clears the moment a reload lands on the served build — regardless of which
 *  worker controls the page (a claimed-but-not-reloaded SW reports the new id
 *  while the page still runs the old bundle, and a cache-busted reload can load
 *  the new bundle under an old controller). It is SELF-CORRECTING: every
 *  definitive check sets the badge true or false, so a transient false-positive
 *  can't latch it on forever the way a one-way `= true` did.
 *
 *  Runs on startup, on resume, and after a SW swap (controllerchange + the
 *  activated statechange). Two guards keep it from clearing a legitimately-lit
 *  badge on noise: the dev placeholder (`__…__`) carries no signal, and an
 *  indeterminate served id (offline / transient — `getServedBuildId` returns
 *  null) leaves the badge as is rather than mis-clearing it; the next check
 *  re-evaluates. */
export async function syncClientUpdateFromBuild(): Promise<void> {
  if (CLIENT_BUILD_ID.startsWith('__')) return; // un-stamped dev build — no signal
  const served = await getServedBuildId();
  if (served === null) return; // couldn't determine — leave the badge unchanged
  const stale = served !== CLIENT_BUILD_ID;
  updateAvailable.value = stale;
  if (stale) surfaceUpdateToast(served);
}
