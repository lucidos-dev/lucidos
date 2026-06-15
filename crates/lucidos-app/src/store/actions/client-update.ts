import { updateAvailable } from '../store';
import { getServedBuildId } from '../../hooks/sw-update';
import { CLIENT_BUILD_ID } from 'virtual:build-id';

/** Sync the "client update available" badge to whether the RUNNING code is
 *  older than the build the server is serving — `CLIENT_BUILD_ID` (the build
 *  that produced the code executing right now, stamped into the bundle at build
 *  time) vs the served `/sw.js` BUILD_ID (`getServedBuildId`).
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
 *  Runs on startup, on resume, and after a SW swap (controllerchange). Two
 *  guards keep it from clearing a legitimately-lit badge on noise: the dev
 *  placeholder (`__…__`) carries no signal, and an indeterminate served id
 *  (offline / transient — `getServedBuildId` returns null) leaves the badge as
 *  is rather than mis-clearing it; the next check re-evaluates. */
export async function syncClientUpdateFromBuild(): Promise<void> {
  if (CLIENT_BUILD_ID.startsWith('__')) return; // un-stamped dev build — no signal
  const served = await getServedBuildId();
  if (served === null) return; // couldn't determine — leave the badge unchanged
  updateAvailable.value = served !== CLIENT_BUILD_ID;
}
