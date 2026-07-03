import { updateAvailable, showToast, removeToast, dismissToast, preferences } from '../store';
import {
  getServedBuildId,
  refreshClient,
  wasSwUpdateDismissed,
  noteUpdateBuildId,
} from '../../hooks/sw-update';
import { CLIENT_BUILD_ID } from 'virtual:build-id';

const UPDATE_TOAST_KEY = 'update-available';

/** Show the "New version available → Refresh" toast. Called by
 *  `syncClientUpdateFromBuild` only when a refresh is genuinely available (stale
 *  AND not already dismissed for this served build), so it has no guards of its
 *  own — on ARRIVAL the badge and this toast surface together from the ONE
 *  upstream signal (INV-B). Dismissing (the X or "Later") only defers: it hides
 *  the toast and remembers this build, but the badge stays lit so the user can
 *  still refresh from it — hence the explicit "Later" affordance. */
function surfaceUpdateToast(servedBuildId: string): void {
  // Record which build the toast offers so a key-only dismiss (the Toast close
  // button or "Later" → dismissToast('update-available')) pins the right id.
  noteUpdateBuildId(servedBuildId);
  showToast('New version available — refresh to sync', 'info', {
    key: UPDATE_TOAST_KEY,
    // "Later" defers the refresh: same path as the X — remembers this build and
    // hides the toast, while the reload badge stays lit as the update affordance.
    secondaryAction: {
      label: 'Later',
      onClick: () => { dismissToast(UPDATE_TOAST_KEY); },
    },
    action: {
      label: 'Refresh',
      onClick: () => {
        // Collapse this toast structurally (removeToast, NOT dismissToast):
        // clicking Refresh is ACTING on the prompt, not deferring it, so it must
        // NOT write the now-workspace-global `client_refresh_dismissed_build`
        // preference — that would suppress the refresh toast on OTHER devices
        // still running the stale bundle that haven't refreshed. Mirrors
        // initiateEngineRestart's acting-not-deferring rule. If this device's
        // reload lands still-stale, syncClientUpdateFromBuild re-surfaces it,
        // which is correct.
        removeToast(UPDATE_TOAST_KEY);
        // SW-aware: a bare reload keeps the current service worker, so it won't
        // pick up the new sw.js. refreshClient swaps to the new worker (or busts
        // the shell cache if it won't), so the badge clears on the next load.
        refreshClient();
      },
    },
  });
}

/** Sync the client-update **badge AND Refresh toast** — both from ONE signal — to
 *  whether the RUNNING code is older than the build the server is serving:
 *  `CLIENT_BUILD_ID` (the build that produced the code executing right now) vs the
 *  served `/sw.js` BUILD_ID (`getServedBuildId`).
 *
 *  The engine only ever serves a client compatible with itself: in dev it serves
 *  a boot-pinned `dist/` snapshot (see `api/frontend_snapshot.rs`), so a stale
 *  loaded bundle means a *newer, compatible* client is being served — a refresh
 *  is both wanted and safe. There is therefore no engine-pending gate here: the
 *  serving layer, not this check, upholds "never a client for a non-running
 *  engine" (incl. on reload). Comparing the LOADED bundle (not the controlling
 *  worker's id) is the honest signal: it stays lit exactly while the running code
 *  is stale and clears the moment a reload lands on the served build.
 *
 *  Badge ⟺ toast on ARRIVAL: the badge is `stale`, and when a stale build is
 *  first seen (not yet dismissed) the toast surfaces alongside it — so a lit
 *  badge is always accompanied by the toast *on arrival* (INV-B). The two
 *  decouple only on DISMISS: dismissing hides the toast and remembers this build
 *  (`wasSwUpdateDismissed`), but the badge stays lit as the persistent refresh
 *  affordance until the client is no longer stale. A genuinely newer served
 *  build re-surfaces the toast. Self-correcting: every definitive check re-derives
 *  the badge from staleness, so a transient false-positive can't latch. Runs on
 *  startup, on resume, after a SW swap (controllerchange + the activated
 *  statechange), and after an engine switch (connection.ts). Two guards keep it
 *  from clobbering a legitimately-lit badge on noise: the dev placeholder
 *  (`__…__`) carries no signal, and an indeterminate served id (offline /
 *  transient — `getServedBuildId` returns null) leaves things as-is. */
export async function syncClientUpdateFromBuild(): Promise<void> {
  if (CLIENT_BUILD_ID.startsWith('__')) return; // un-stamped dev build — no signal
  // The refresh dismissal is now a GLOBAL preference (not synchronous
  // localStorage), so until preferences load we can't tell whether this build was
  // already dismissed. Skip rather than fail-open into a flash of an
  // already-dismissed toast on cold start — useStartup re-runs this right after
  // loadPreferences, and resume / PreferencesChanged re-derive thereafter. A
  // 'failed' load proceeds (fail-open surfacing is the safe default).
  if (preferences.value.status === 'not-loaded' || preferences.value.status === 'loading') return;
  const served = await getServedBuildId();
  if (served === null) return; // couldn't determine — leave badge + toast unchanged
  const stale = served !== CLIENT_BUILD_ID;
  // Badge = staleness alone: the persistent "refresh available" affordance that
  // survives a dismiss (the user can still refresh from the reload badge).
  updateAvailable.value = stale;
  // Toast = staleness AND not-yet-dismissed-for-this-build: the one-time
  // announcement. On arrival (stale, not dismissed) it appears with the badge;
  // a dismiss defers it (badge stays), and a genuinely newer build re-surfaces it.
  if (stale && !wasSwUpdateDismissed(served)) {
    surfaceUpdateToast(served);
  } else {
    // In sync, or deferred for this build → hide the toast (badge stays if still
    // stale). Structural removal only — never a user-dismiss (that would wrongly
    // pin this build as dismissed).
    removeToast(UPDATE_TOAST_KEY);
  }
}
