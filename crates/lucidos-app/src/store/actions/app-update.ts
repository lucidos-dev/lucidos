import { showToast, removeToast, latestTauriAppVersion, latestTauriAppNotes, appUpdateCheckError, appUpdateCheckInFlight, appUpdateProgress, releaseCheck, settingsScrollTarget } from '../store';
// The READ lives a layer up, where a surface can ask "is there an update?"
// without importing this module's toasts, IPC and menu navigation.
import { packagedUpdateVersion } from '../packagedUpdate';
import { openWhatsNew, openSettingsSubview } from './menu';
import { isTauri } from '../../utils/platform';
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

/** Why a check could not run, when the INSTALL is the reason rather than the
 *  session. A source checkout is the everyday case, and it never polls. */
const NO_CHECK_HERE = 'this install has no update check';

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
/** The user-initiated check in flight, or `null`. Holding the PROMISE is what
 *  makes a second caller join the first rather than race it: a bare boolean
 *  would let the loser resolve with nothing. See {@link checkForUpdatesNow}. */
let inFlightCheck: Promise<UpdateCheckVerdict> | null = null;
/** Whether {@link latestTauriAppVersion} holds a value the CLIENT check wrote.
 *
 *  Gates the clear-on-no-update path so the client check never wipes a value it
 *  did not put there. In a Tauri dev client `check_app_update` returns null,
 *  which is indistinguishable from "up to date". Assigning that null would
 *  clobber the version `connection.ts` reads from the engine's `/health`. */
let clientOwnsLatestVersion = false;

/** What a check concluded, as the thing the CALLER acts on.
 *
 *  The verdict travels as a return value rather than being re-read off the
 *  signals afterwards. Those signals are also written by the background poll,
 *  so a reader could otherwise report one request's state having awaited
 *  another's. That is what showed "Lucidos is up to date" a moment before the
 *  offer toast for the release it had just found. */
export type UpdateCheckVerdict =
  /** A newer release is available to this install. */
  | { kind: 'available'; version: string }
  /** The check ran and found nothing newer. */
  | { kind: 'up-to-date' }
  /** The check failed, or there was none this session could run. `reason` is
   *  user-facing. */
  | { kind: 'failed'; reason: string }
  /** An install is already under way, so its own narration is the answer. */
  | { kind: 'installing' };

/** Surface the "Lucidos <v> available" offer.
 *
 *  It always carries an action, which is the rule this whole module keeps: no
 *  surface may announce a release without saying how to get it. Which action is
 *  {@link updateRoute}'s answer, so the toast, What's New and Settings cannot
 *  disagree about what taking this update takes.
 *
 *  The wording is sentence case, as every other toast action is. The buttons in
 *  Settings are Title Case, as every other `.action-btn` is. One route, two
 *  typographic conventions.
 *
 *  Extracted so the cancel path can put the offer straight back: abandoning a
 *  download abandons the attempt, not the update. */
function offerAppUpdate(version: string): void {
  const route = updateRoute(true);
  const action = {
    label: route === 'install' ? 'Update & restart' : 'How to update',
    onClick: () => { void followUpdateRoute(route); },
  };
  showToast(`Lucidos ${version} available`, 'info', {
    key: UPDATE_TOAST_KEY,
    action,
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
    if (version) offerAppUpdate(version);
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
export async function refreshReleaseCheck(force = false): Promise<UpdateCheckVerdict> {
  try {
    releaseCheck.value = await requestUpdateCheck(force);
  } catch (e) {
    // No gateway (a direct engine port), an older one with no such route, or a
    // transient blip. Leaves the last known answer standing and the next resume
    // retries. Settings → System falls back to the client check while the
    // gateway announces nothing at all.
    console.warn('[app-update] gateway release check unavailable; retried on next resume', e);
    const reason = errorDetail(e);
    if (force) appUpdateCheckError.value = reason;
    return { kind: 'failed', reason };
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
  // A gateway that MAY not poll returns an untouched snapshot, and reading that
  // as "up to date" is a claim nothing supports: it never asked. `supported` is
  // false for a source checkout, and for a target we publish nothing for.
  // `may_poll` refuses both however hard the caller forces (ADR 0108).
  if (!releaseCheck.value.supported) return { kind: 'failed', reason: NO_CHECK_HERE };
  if (!latest) {
    // A stale answer plus a failed poll is not "up to date": the gateway has
    // nothing newer to report BECAUSE it could not ask.
    const lastError = releaseCheck.value.last_error;
    return lastError ? { kind: 'failed', reason: lastError } : { kind: 'up-to-date' };
  }
  // An update already running owns the shared toast key, and its narration is
  // a far more specific answer than a fresh offer would be.
  if (appUpdateProgress.value) return { kind: 'installing' };
  // The dedupe keeps a repeat BACKGROUND poll from re-offering on every resume.
  // A forced check is a click, and a click is owed a reply: without the bypass,
  // asking again after dismissing the toast produced nothing at all.
  if (lastOfferedVersion !== latest.version || force) {
    lastOfferedVersion = latest.version;
    offerAppUpdate(latest.version);
  }
  return { kind: 'available', version: latest.version };
}

/** The single user-initiated check, shared by every control that offers one.
 *
 *  It owns three things no caller should re-derive. The in-flight signal, so
 *  the control can report itself and refuse a second start. The single flight,
 *  so two overlapping clicks make one request and share one answer. And the
 *  verdict, returned rather than read back off the signals.
 *
 *  The gateway owns the check (ADR 0108), so that is the first choice. The
 *  client's own Tauri updater is the ADR 0105 fallback. It is taken only where
 *  the gateway announces nothing at all, which means one too old to carry the
 *  field. */
export function checkForUpdatesNow(): Promise<UpdateCheckVerdict> {
  if (inFlightCheck) return inFlightCheck;
  appUpdateCheckInFlight.value = true;
  inFlightCheck = runUserCheck().finally(() => {
    inFlightCheck = null;
    appUpdateCheckInFlight.value = false;
  });
  return inFlightCheck;
}

/** What a surface can offer about a release newer than the one running.
 *
 *  Three answers, and deliberately never "nothing" (ADR 0142). A surface may
 *  not say a newer release exists and then leave the reader no way to get it.
 *  That is what What's New did for every release the updater had not offered.
 *
 *  - `install`: take it here. A Tauri client fronting a bundle.
 *  - `check`: no offer yet, and this session has a check it can run.
 *  - `guide`: the answer is on Settings, System. It carries the installer
 *    command for a headless install, and the rebuild for a source checkout. */
export type UpdateRoute = 'install' | 'check' | 'guide';

/** Could this session install an offer, if one existed?
 *
 *  A Tauri client fronting a bundle can. A browser or PWA session has no IPC to
 *  invoke, and a headless install is updated by re-running `install.sh`.
 *
 *  It SUBTRACTS `installer-rerun` rather than requiring `desktop-app`, and the
 *  asymmetry is deliberate. `install` describes the GATEWAY's own layout, while
 *  `isTauri()` is what answers "can this session install". A Tauri client is a
 *  bundle by construction. Requiring `desktop-app` would only withhold a working
 *  button: from a layout the gateway failed to recognise, and from a dev client
 *  with no `latest`. See `docs/code-review-priors.md`. */
export function sessionCanInstall(): boolean {
  return isTauri() && releaseCheck.value?.latest?.install !== 'installer-rerun';
}

/** Can this session ask, right now, whether a newer release is published?
 *
 *  The gateway is authoritative once it has answered. `supported` is false for a
 *  source checkout and for a target Lucidos publishes nothing for, and
 *  `may_poll` refuses either way (ADR 0108). Offering a check there would be a
 *  button that errors every time it is pressed.
 *
 *  With no gateway answer at all, the client's own updater is the ADR 0105
 *  fallback. That covers a gateway too old to carry the field, and a direct
 *  engine port with no gateway in front of it. */
export function canCheckForUpdatesHere(): boolean {
  const check = releaseCheck.value;
  return check ? check.supported : isTauri();
}

/** Which route this session takes to a newer release.
 *
 *  `offered` says whether there is an offer to act on. It defaults to the
 *  updater's own answer, and a caller holding an offer already passes `true`.
 *  That matters on the client-check path, where the offer is in hand before the
 *  signals it would be re-derived from have settled. */
export function updateRoute(offered: boolean = packagedUpdateVersion() !== null): UpdateRoute {
  if (offered) return sessionCanInstall() ? 'install' : 'guide';
  return canCheckForUpdatesHere() ? 'check' : 'guide';
}

/** The label an update BUTTON wears, in all four of its states.
 *
 *  Every word here, so Settings and What's New cannot drift apart. Handing each
 *  surface its own idle string is what let them ship as "Check for Updates" and
 *  "Check for updates" at once.
 *
 *  Paired with `disabled`, the in-flight word is also the whole of the feedback
 *  a fast check needs. A spinner would be a second gate on top of this one. */
export function updateControlLabel(route: UpdateRoute, checking: boolean): string {
  if (checking) return 'Checking…';
  if (route === 'install') return 'Update & Restart';
  return route === 'check' ? 'Check for Updates' : 'How to Update';
}

/** Do what a route says. The click behind every label above.
 *
 *  `guide` lands on Settings, System and scrolls to Maintenance, which is where
 *  the installer command, the update button and the rebuild control all live.
 *  Nothing else in the app holds the whole account of how an install updates. */
export async function followUpdateRoute(route: UpdateRoute): Promise<void> {
  if (route === 'install') {
    await installAppUpdate();
    return;
  }
  if (route === 'check') {
    reportUpdateCheck(await checkForUpdatesNow());
    return;
  }
  settingsScrollTarget.value = 'system:maintenance';
  openSettingsSubview('system');
}

/** Say what a user-initiated check concluded.
 *
 *  One wording for every control that offers a check, so Settings and What's
 *  New cannot answer the same click differently. `available` and `installing`
 *  are deliberately silent: the offer toast and the progress dialog have
 *  already said more than a second toast could. */
export function reportUpdateCheck(verdict: UpdateCheckVerdict): void {
  if (verdict.kind === 'up-to-date') showToast('Lucidos is up to date', 'success');
  else if (verdict.kind === 'failed') {
    showToast(`Couldn't check for updates: ${verdict.reason}`, 'error');
  }
}

async function runUserCheck(): Promise<UpdateCheckVerdict> {
  if (appUpdateProgress.value) return { kind: 'installing' };
  if (releaseCheck.value) return refreshReleaseCheck(true);
  if (isTauri()) return checkAppUpdateViaClient();
  // No gateway answer and no client updater: a browser or PWA session on a
  // direct engine port. It cannot learn about a release at all, so "up to date"
  // would be a guess dressed up as an answer.
  return { kind: 'failed', reason: 'this session has no update check to run' };
}

/** Ask the Tauri updater directly. The ADR 0105 degradation for a gateway too
 *  old to announce a release: the client is newer than the machine's gateway
 *  right after an update, and this keeps Settings → System working meanwhile.
 *
 *  User-initiated only, and on no timer. The outcome is recorded so the
 *  persistent Settings surface can report a failed check rather than let it look
 *  like "you are up to date". */
export async function checkAppUpdateViaClient(): Promise<UpdateCheckVerdict> {
  if (!isTauri()) return { kind: 'failed', reason: 'no client updater in this session' };
  if (appUpdateProgress.value) return { kind: 'installing' };
  let offer: AppUpdateOffer | null;
  try {
    offer = await checkAppUpdate();
  } catch (e) {
    // Through `errorDetail` because the rejection is no longer always Rust's own
    // error string: an unreadable IPC payload arrives as an Error, and `String`
    // would put its "Error: " prefix in front of the reason on the System page.
    const reason = errorDetail(e);
    appUpdateCheckError.value = reason;
    return { kind: 'failed', reason };
  }
  appUpdateCheckError.value = null;
  // The notes travel WITH the version, written and cleared on the same branches,
  // so the two can never end up describing different releases. Clear only what
  // this path set, see {@link clientOwnsLatestVersion}.
  if (offer) {
    latestTauriAppVersion.value = offer.version;
    latestTauriAppNotes.value = offer.notes;
    clientOwnsLatestVersion = true;
    offerAppUpdate(offer.version);
    return { kind: 'available', version: offer.version };
  }
  if (clientOwnsLatestVersion) {
    latestTauriAppVersion.value = null;
    latestTauriAppNotes.value = null;
    clientOwnsLatestVersion = false;
  }
  return { kind: 'up-to-date' };
}

/** Can THIS session install the offered update itself?
 *
 *  An offer exists for every install shape, but only a Tauri client fronting a
 *  bundle can act on one. Both halves: there must BE an offer, and the session
 *  must be able to take it. {@link sessionCanInstall} owns the second half and
 *  documents why it subtracts `installer-rerun`. */
export function canInstallUpdateHere(): boolean {
  return packagedUpdateVersion() !== null && sessionCanInstall();
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
