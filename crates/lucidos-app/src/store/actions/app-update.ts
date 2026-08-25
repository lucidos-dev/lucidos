import { showToast, removeToast, latestTauriAppVersion, latestTauriAppNotes, appUpdateCheckError, appUpdateProgress, releaseCheck } from '../store';
import { openWhatsNew, openSettingsSubview } from './menu';
import { isTauri } from '../../utils/platform';
import { isNewerVersion } from '../../utils/version';
import { errorDetail } from '../../utils/errorDetail';
import { requestUpdateCheck } from '../../api/client/control';
import {
  checkAppUpdate,
  installAppUpdateAndRestart,
  listen,
  APP_UPDATE_PROGRESS_EVENT,
  type AppUpdateOffer,
  type AppUpdateProgress,
} from '../../utils/tauri';

/** ONE key for every packaged-update TOAST: the "Lucidos <v> available" offer,
 *  and a failure. Keyed, so a failure replaces the offer in place rather than
 *  stacking a second toast on it.
 *
 *  The live progress is not here. A run takes the app away and comes back with
 *  a different one, so it owns the progress dialog instead
 *  (docs/plans/2026-08-13-toast-banner-dialog-taxonomy.md). */
const UPDATE_TOAST_KEY = 'app-update-available';

let installing = false;
/** Unsubscribe for the progress event, or `null` when not subscribed. */
let unlistenProgress: (() => void) | null = null;
/** Guards the async gap in {@link startAppUpdateProgress} so remounting
 *  (reload, workspace switch, restart-reconnect) can't register a second
 *  listener while the first `listen` call is still in flight. */
let subscribing = false;
/** The version this window has already toasted about, so a repeat refresh does
 *  not raise a second offer for the same release. The same dedupe the plugin
 *  marketplace scan keeps in `plugin-update-notice.json`, one layer up. */
let lastOfferedVersion: string | null = null;
/** Whether {@link latestTauriAppVersion} holds a value the CLIENT check wrote.
 *
 *  Gates the clear-on-no-update path so the client check never wipes a value it
 *  did not put there. In a Tauri dev client `check_app_update` returns null,
 *  which is indistinguishable from "up to date". Assigning that null would
 *  clobber the version `connection.ts` reads from the engine's `/health`. */
let clientOwnsLatestVersion = false;

/** Surface the "Lucidos <v> available" offer.
 *
 *  The install action appears only where an install is possible: a Tauri client
 *  fronting a `desktop-app` install. A browser or PWA session can install
 *  nothing, so it gets the version and no button. A headless install gets a
 *  route to the command instead, which lives in Settings, System.
 *
 *  Extracted so the cancel path can put the offer straight back: abandoning a
 *  download abandons the attempt, not the update. */
function offerAppUpdate(version: string, install?: string | null): void {
  // One derivation of what this session can do about the offer. Two spreads
  // that both wrote `action` could not express that.
  const action = isTauri() && install !== 'installer-rerun'
    ? { label: 'Update & restart', onClick: () => { void installAppUpdate(); } }
    : install === 'installer-rerun'
      ? { label: 'How to update', onClick: () => { openSettingsSubview('system'); } }
      : null;
  showToast(`Lucidos ${version} available`, 'info', {
    key: UPDATE_TOAST_KEY,
    ...(action ? { action } : {}),
    // "What is in it?" is the question an update offer raises, and answering it
    // is not the same click as taking the update. Rendered only when notes are
    // known, so the control never opens onto nothing. The gateway carries none,
    // so this is the client-check path's affordance.
    //
    // It NAMES the version, which is what makes the panel open on it. Left
    // unnamed, What's New falls back to expanding the release already running.
    ...(latestTauriAppNotes.value
      ? {
          secondaryAction: {
            label: "What's new",
            onClick: () => { openWhatsNew(version); },
          },
        }
      : {}),
  });
}

/** Take one progress frame. A running frame is simply RECORDED: the progress
 *  dialog is derived from {@link appUpdateProgress}, so recording the frame is
 *  what draws it, and clearing the signal is what closes it.
 *
 *  A terminal frame ends the run and says why on a toast. A finished operation
 *  is a message, rather than something the user must watch. */
function handleAppUpdateProgress(frame: AppUpdateProgress): void {
  if (frame.phase === 'cancelled') {
    appUpdateProgress.value = null;
    installing = false;
    // Nothing was written to disk, so the update is still available — re-offer it
    // rather than leaving the user with a silently vanished toast.
    const version = frame.version ?? packagedUpdateVersion();
    if (version) offerAppUpdate(version, releaseCheck.value?.latest?.install ?? null);
    else removeToast(UPDATE_TOAST_KEY);
    return;
  }
  if (frame.phase === 'failed') {
    // Clear the run BEFORE toasting: `appUpdateCommitted` suppresses ordinary
    // toasts while an update is past the point of no return, and this error is
    // exactly the thing that must get through.
    appUpdateProgress.value = null;
    installing = false;
    showToast(`Update failed: ${frame.message}`, 'error', { key: UPDATE_TOAST_KEY });
    return;
  }
  if (frame.phase === 'bundle-swap-failed') {
    // The install destroyed the app on disk without landing a replacement, so
    // this is not "the update failed, try again": there is nothing left to try
    // against. The Rust message is already a full account with the recovery path
    // in it, so it is shown verbatim rather than prefixed, and the update is
    // deliberately NOT re-offered the way a cancel re-offers it. Same
    // clear-before-toast ordering as `failed`, for the same reason.
    appUpdateProgress.value = null;
    installing = false;
    showToast(frame.message, 'error', { key: UPDATE_TOAST_KEY });
    return;
  }
  // A running frame. The dialog reads this signal, applies the narration, and
  // offers Cancel exactly while `cancellable` says one can still work. Nothing
  // needs keeping alive against the toast suppression either, since a dialog is
  // not a toast. That matters from `installing` on, where the launchd service
  // restarts and the gateway serving this page dies with it.
  appUpdateProgress.value = frame;
  // Drop the offer the run started from. The keyed toast used to be REPLACED by
  // the narration. Now that the narration is a dialog, the offer would sit
  // behind it, still inviting a click on an update already running.
  removeToast(UPDATE_TOAST_KEY);
}

/** Subscribe to the Rust updater's progress stream. Idempotent across remounts;
 *  Tauri-only. Carries no timer: the CHECK lives in the gateway (ADR 0108) and
 *  this is only the narration for an install the user started.
 *
 *  Best-effort (frontend.md carve-out): this runs at startup without user intent,
 *  and a failed subscription costs the narration, not the update — the install
 *  action's own error toast and Settings → System still report outcomes. It
 *  leaves itself unsubscribed on failure, so the next mount retries. */
export function startAppUpdateProgress(): void {
  if (!isTauri()) return;
  void subscribeToAppUpdateProgress();
}

async function subscribeToAppUpdateProgress(): Promise<void> {
  if (unlistenProgress || subscribing) return;
  subscribing = true;
  try {
    unlistenProgress = await listen<AppUpdateProgress>(
      APP_UPDATE_PROGRESS_EVENT,
      (e) => { handleAppUpdateProgress(e.payload); },
    );
  } catch (e) {
    console.warn('[app-update] progress subscription failed; retried on next mount', e);
  } finally {
    subscribing = false;
  }
}

/** Drop the progress subscription (startup cleanup). */
export function stopAppUpdateProgress(): void {
  if (unlistenProgress) {
    unlistenProgress();
    unlistenProgress = null;
  }
}

/** Ask the GATEWAY whether a newer Lucidos is published, and record the answer.
 *
 *  The check itself lives in the gateway (ADR 0108), so this is a loopback read
 *  rather than a poll of the release host. The gateway asks lucidos.dev only
 *  when its own answer is stale, and concurrent callers coalesce there, so N
 *  open windows still cost one outbound request.
 *
 *  Best-effort on the background path (frontend.md carve-out): it runs on mount
 *  and on resume without user intent, so a transport failure is a
 *  `console.warn`. A FORCED call is the Settings button, which the user clicked
 *  and is owed an answer to, so that one records the failure.
 *
 *  `force` is that button asking for a poll now. */
export async function refreshReleaseCheck(force = false): Promise<void> {
  try {
    releaseCheck.value = await requestUpdateCheck(force);
  } catch (e) {
    // No gateway (a direct engine port), an older one with no such route, or a
    // transient blip. Leaves the last known answer standing and the next resume
    // retries. Settings → System falls back to the client check while the
    // gateway announces nothing at all.
    console.warn('[app-update] gateway release check unavailable; retried on next resume', e);
    if (force) appUpdateCheckError.value = errorDetail(e);
    return;
  }
  // A poll that FAILED must never read as "you are up to date", so the
  // gateway's own verdict drives the persistent Settings notice. Cleared by
  // the same field on the next success.
  appUpdateCheckError.value = releaseCheck.value.last_error;
  const latest = releaseCheck.value.latest;
  // The notes travel with the version, so the "What's new" link and the panel's
  // Available row cannot end up describing a different release. The origin may
  // carry none, and then there is simply no link.
  latestTauriAppNotes.value = latest?.notes ?? null;
  if (!latest) return;
  if (lastOfferedVersion === latest.version) return;
  // An update already running owns the shared toast key, and its narration is
  // a far more specific answer than a fresh offer would be.
  if (appUpdateProgress.value) return;
  lastOfferedVersion = latest.version;
  offerAppUpdate(latest.version, latest.install);
}

/** Ask the Tauri updater directly. The ADR 0105 degradation for a gateway too
 *  old to announce a release: the client is newer than the machine's gateway
 *  right after an update, and this keeps Settings → System working meanwhile.
 *
 *  User-initiated only, and on no timer. The outcome is recorded so the
 *  persistent Settings surface can report a failed check rather than let it look
 *  like "you are up to date". */
export async function checkAppUpdateViaClient(): Promise<void> {
  if (!isTauri()) return;
  if (appUpdateProgress.value) return;
  let offer: AppUpdateOffer | null;
  try {
    offer = await checkAppUpdate();
  } catch (e) {
    // Through `errorDetail` because the rejection is no longer always Rust's own
    // error string: an unreadable IPC payload arrives as an Error, and `String`
    // would put its "Error: " prefix in front of the reason on the System page.
    appUpdateCheckError.value = errorDetail(e);
    return;
  }
  appUpdateCheckError.value = null;
  // The notes travel WITH the version, written and cleared on the same branches,
  // so the two can never end up describing different releases. Clear only what
  // this path set, see {@link clientOwnsLatestVersion}.
  if (offer) {
    latestTauriAppVersion.value = offer.version;
    latestTauriAppNotes.value = offer.notes;
    clientOwnsLatestVersion = true;
    offerAppUpdate(offer.version, 'desktop-app');
  } else if (clientOwnsLatestVersion) {
    latestTauriAppVersion.value = null;
    latestTauriAppNotes.value = null;
    clientOwnsLatestVersion = false;
  }
}

/** The newer version available to install, or `null`.
 *
 *  One derivation of "is there an update?", shared by the notice, the button
 *  label and the button's action, so the three cannot drift apart. It reads the
 *  signals at call time, so calling it during render subscribes the caller.
 *
 *  The gateway's answer wins where there is one, because it covers every install
 *  shape. The fallback covers two cases. A dev client reads
 *  `latestTauriAppVersion` from the engine's `/health`, and a client on an older
 *  gateway reads its own Tauri check. */
export function packagedUpdateVersion(): string | null {
  const announced = releaseCheck.value?.latest;
  if (announced) return announced.version;
  const latest = latestTauriAppVersion.value;
  const current = window.__LUCIDOS_APP_VERSION__;
  return latest && current && isNewerVersion(latest, current) ? latest : null;
}

/** Can THIS session install the offered update itself?
 *
 *  An offer exists for every install shape, but only a Tauri client fronting a
 *  bundle can act on one. A browser or PWA session has no IPC to invoke, and a
 *  headless install is updated by re-running `install.sh`. Both would otherwise
 *  reach the Tauri updater through a button labelled "Check for Updates".
 *
 *  One derivation, shared by the button's label and by what its click does.
 *
 *  It SUBTRACTS `installer-rerun` rather than requiring `desktop-app`, and the
 *  asymmetry is deliberate. `install` describes the GATEWAY's own layout, while
 *  `isTauri()` is what answers "can this session install". A Tauri client is a
 *  bundle by construction. Requiring `desktop-app` would only withhold a working
 *  button: from a layout the gateway failed to recognise, and from a dev client
 *  with no `latest`. See `docs/code-review-priors.md`. */
export function canInstallUpdateHere(): boolean {
  if (!packagedUpdateVersion() || !isTauri()) return false;
  return releaseCheck.value?.latest?.install !== 'installer-rerun';
}

/** Install the available packaged update and restart the whole stack. This runs on
 *  USER intent (the toast action / the System page button), so a failure is
 *  surfaced as an error toast. On success the client re-execs and this page is
 *  torn down — no further code runs.
 *
 *  The invoke resolves only when the whole thing is OVER; everything the user sees
 *  in between arrives on the progress event (see {@link handleAppUpdateProgress}).
 *  Awaiting it silently is precisely what made the update look like a freeze. */
export async function installAppUpdate(): Promise<void> {
  if (installing) return;
  installing = true;
  // React on the CLICK, not on the first event: the IPC hop plus the updater's own
  // network check take long enough that the button would otherwise look dead.
  handleAppUpdateProgress({ version: packagedUpdateVersion(), phase: 'checking' });
  try {
    await installAppUpdateAndRestart();
    // Reached only if the command resolved WITHOUT restarting — a cancel (whose
    // event already reset the surface) or a no-op. Allow a retry either way.
    installing = false;
    // The command has returned, so by definition nothing is running any more: a
    // still-set run here would be a spinner with nothing behind it. Normally the
    // cancel frame has already cleared it, but the event and the command reply
    // travel on different channels and can land in either order.
    if (appUpdateProgress.value) {
      appUpdateProgress.value = null;
      removeToast(UPDATE_TOAST_KEY);
    }
  } catch (e) {
    installing = false;
    // Rust emits a `failed` frame for everything it can attribute, and that
    // handler has already cleared the run and shown the reason. This covers what
    // it cannot reach — a rejected invoke, an ACL denial, a dead bridge — so a
    // failed update can never leave the toast spinning forever.
    if (appUpdateProgress.value) {
      appUpdateProgress.value = null;
      showToast(`Update failed: ${errorDetail(e)}`, 'error', { key: UPDATE_TOAST_KEY });
    }
  }
}
