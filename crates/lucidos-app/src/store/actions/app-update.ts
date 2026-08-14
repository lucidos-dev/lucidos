import { showToast, removeToast, latestTauriAppVersion, latestTauriAppNotes, appUpdateCheckError, appUpdateProgress } from '../store';
import { openSettingsSubview } from './menu';
import { isTauri } from '../../utils/platform';
import { isNewerVersion } from '../../utils/version';
import { errorDetail } from '../../utils/errorDetail';
import { formatBytes } from '../../utils/formatBytes';
import {
  checkAppUpdate,
  installAppUpdateAndRestart,
  cancelAppUpdate,
  listen,
  APP_UPDATE_PROGRESS_EVENT,
  type AppUpdateOffer,
  type AppUpdateProgress,
  type AppUpdateRunning,
} from '../../utils/tauri';

/** How often the packaged client re-checks for an app update. The client is
 *  long-resident (the window can be closed while it stays alive in the menu bar),
 *  so a launch-only check would miss an update published mid-session.
 *
 *  An hour, not the 6h this used to be: releases can land minutes apart, and a
 *  client launched shortly before one spent most of a working day claiming to be
 *  current. That is exactly what happened on 2026-07-31. A 0.18.0 client started
 *  at 08:54, 0.18.1 was published at 09:16 and 0.18.2 at 10:22, and the next
 *  unattended check was not due until 14:54. */
const APP_UPDATE_POLL_MS = 60 * 60 * 1000; // 1h

/** Floor between two RESUME-triggered checks. `focus` / `visibilitychange` fire
 *  on every window switch, and each check is a network round-trip to the release
 *  host, so an unthrottled per-resume check would hammer it to say nothing new.
 *  Five minutes is long enough that flicking between windows costs nothing, and
 *  short enough that coming back to a client left running resolves before the
 *  user could notice the wait. */
const APP_UPDATE_RESUME_MIN_INTERVAL_MS = 5 * 60 * 1000;

/** ONE key for the whole packaged-update surface: the "Lucidos <v> available"
 *  offer, the live progress narration, and a failure. Keyed toasts update in
 *  place, so the offer MORPHS into the progress readout instead of a second toast
 *  stacking on top of it — the same single-surface discipline
 *  `initiateEngineRestart` applies to the engine switch. */
const UPDATE_TOAST_KEY = 'app-update-available';

let pollTimer: ReturnType<typeof setInterval> | null = null;
let installing = false;
/** When the last check actually reached the network (`null` before the first).
 *  Process-lifetime, deliberately NOT reset by {@link stopAppUpdateChecks}: it
 *  records when we last asked the release host, which a remount does not undo.
 *  Read only by {@link recheckAppUpdateOnResume}; the interval keeps its own
 *  fixed cadence. */
let lastCheckStartedAt: number | null = null;
/** Unsubscribe for the progress event, or `null` when not subscribed. */
let unlistenProgress: (() => void) | null = null;
/** Guards the async gap in {@link subscribeToAppUpdateProgress} so remounting
 *  (reload, workspace switch, restart-reconnect) can't register a second
 *  listener while the first `listen` call is still in flight. */
let subscribing = false;
/** Whether {@link latestTauriAppVersion} currently holds a value THIS module
 *  wrote. Gates the clear-on-no-update path so the updater never wipes a value
 *  it did not put there: in a Tauri DEV client `check_app_update` is a no-op
 *  returning null, which is indistinguishable from "up to date" — and blindly
 *  assigning that null would clobber the version `connection.ts` reads from the
 *  engine's `/health`, which is the only source dev has. The two would then
 *  fight on every poll. */
let updaterOwnsLatestVersion = false;

/** How the update surface should read for a given progress frame: the sentence,
 *  the determinate fraction (or `null` when there is no honest one), and whether
 *  the run can still be abandoned.
 *
 *  Pure, and the SINGLE derivation behind both the toast and Settings → System —
 *  the two must never disagree about what the update is doing. Terminal frames
 *  (`cancelled` / `failed`) are deliberately absent: they end the run rather than
 *  describe it, so their callers replace the toast instead of updating it. */
export interface AppUpdateNarration {
  message: string;
  /** Determinate fraction in [0, 1], or `null` when there is no honest one.
   *  Clamped HERE rather than at each renderer: the toast and the System page
   *  both paint it, and a server whose `Content-Length` undercounts the body
   *  would otherwise send one of them past the end of its own track. */
  progress: number | null;
  cancellable: boolean;
}

/** `Lucidos 2026.7.30`, or a version-less fallback for the window before the
 *  check has resolved one. */
function updateLabel(version: string | null): string {
  return version ? `Lucidos ${version}` : 'the update';
}

export function appUpdateNarration(frame: AppUpdateRunning): AppUpdateNarration {
  const name = updateLabel(frame.version);
  switch (frame.phase) {
    case 'checking':
      // Cancellable: the check is a network round-trip that can hang on a bad
      // connection, and the Rust side races it against the cancel signal too.
      return { message: 'Checking for updates…', progress: null, cancellable: true };
    case 'downloading': {
      // No `Content-Length` means no honest percentage — show the bytes moving
      // and let the spinner carry the rest. Never a fabricated bar.
      const { downloaded, total } = frame;
      const sized = total !== null && total > 0;
      return {
        message: `Downloading ${name} — ${formatBytes(downloaded)}${sized ? ` of ${formatBytes(total)}` : ''}`,
        progress: sized ? Math.min(1, downloaded / total) : null,
        cancellable: true,
      };
    }
    case 'verifying':
      // Signature check over the buffered bytes — sub-second, and nothing has
      // touched the disk yet. A cancel WOULD still land here (Rust runs the check
      // inside the abortable download), but a button that appears and vanishes
      // within a few hundred ms is noise, so it is withheld. Hiding a working
      // affordance is safe; the rule is only never to OFFER one that can't work.
      // Full bar: the transfer really is complete.
      return { message: `Verifying ${name}…`, progress: 1, cancellable: false };
    case 'installing':
      return { message: `Installing ${name}…`, progress: null, cancellable: false };
    case 'restarting-services':
      return { message: 'Restarting background services…', progress: null, cancellable: false };
    case 'relaunching':
      return { message: 'Relaunching Lucidos…', progress: null, cancellable: false };
  }
}

/** Surface the "Lucidos <v> available → Update & restart" offer. Extracted so
 *  the cancel path can put it straight back: abandoning a download abandons the
 *  attempt, not the update, and dropping the user back to no affordance at all
 *  would strand them until the next poll. */
function offerAppUpdate(version: string): void {
  showToast(`Lucidos ${version} available`, 'info', {
    key: UPDATE_TOAST_KEY,
    action: {
      label: 'Update & restart',
      onClick: () => { void installAppUpdate(); },
    },
    // "What is in it?" is the question an update offer raises, and answering it
    // is not the same click as taking the update. Rendered only when the
    // manifest actually carried notes, so the control never opens onto nothing.
    // Reads the signal at call time rather than taking a parameter, because the
    // cancel path re-offers with only a version in hand and the notes it should
    // show are the same ones.
    ...(latestTauriAppNotes.value
      ? {
          secondaryAction: {
            label: "What's new",
            onClick: () => { openSettingsSubview('whats-new'); },
          },
        }
      : {}),
  });
}

/** Render one progress frame onto the update surface. Terminal frames end the
 *  run (clearing {@link appUpdateProgress}, which is what re-enables ordinary
 *  toasts) and replace the narration; the rest update it in place. */
function handleAppUpdateProgress(frame: AppUpdateProgress): void {
  if (frame.phase === 'cancelled') {
    appUpdateProgress.value = null;
    installing = false;
    // Nothing was written to disk, so the update is still available — re-offer it
    // rather than leaving the user with a silently vanished toast.
    const version = frame.version ?? latestTauriAppVersion.value;
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
  appUpdateProgress.value = frame;
  const narration = appUpdateNarration(frame);
  showToast(narration.message, 'info', {
    key: UPDATE_TOAST_KEY,
    spinning: true,
    progress: narration.progress,
    // The last phases restart the launchd service, which kills the gateway
    // serving this page; without this the narration would be suppressed by the
    // very disconnection it is explaining.
    showWhileUnavailable: true,
    // Cancel IS the escape hatch while one exists, and once it doesn't there is
    // nothing to escape to — an X beside "Cancel" would just read as a second,
    // differently-behaved cancel.
    dismissable: false,
    secondaryAction: narration.cancellable
      ? { label: 'Cancel', onClick: () => { void cancelAppUpdate(); } }
      : undefined,
  });
}

/** Subscribe to the Rust updater's progress stream. Idempotent across remounts;
 *  Tauri-only.
 *
 *  Best-effort (frontend.md carve-out): this runs at startup without user intent,
 *  and a failed subscription costs the narration, not the update — the install
 *  action's own error toast and Settings → System still report outcomes. It
 *  leaves itself unsubscribed on failure, so the next mount retries. */
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

/** Check for a newer packaged build and, if one exists, surface the in-app
 *  "Update & restart" toast inside the workspace. Tauri-only — a plain browser /
 *  mobile PWA / dev build can't update the desktop app, so this is a no-op there.
 *
 *  The outcome is also RECORDED, not just toasted: {@link latestTauriAppVersion}
 *  and {@link appUpdateCheckError} drive the persistent Settings → System surface.
 *  A toast is transient and dismissable, so it cannot be the only way to reach an
 *  update — and a check that fails must be visible somewhere rather than dying in
 *  a `console.warn` nobody reads (the failure mode that made a stranded install
 *  indistinguishable from an up-to-date one).
 *
 *  Still no error TOAST: this runs on a timer without user intent (frontend.md's
 *  best-effort carve-out). The user-facing failure surfaces are the System page
 *  and the install action's own toast, which does run on user intent. */
export async function checkForAppUpdate(): Promise<void> {
  if (!isTauri()) return;
  // An update already running owns the shared toast key and has a far more
  // specific answer to "is there an update?" than a fresh poll would — re-running
  // one here would overwrite the live narration with a stale offer.
  if (appUpdateProgress.value) return;
  // Stamped after the guards, never before: a call that returned without asking
  // the release host anything must not make the resume throttle think it did.
  lastCheckStartedAt = Date.now();
  let offer: AppUpdateOffer | null;
  try {
    offer = await checkAppUpdate();
  } catch (e) {
    // Recorded for Settings → System; the next poll retries. Through
    // `errorDetail` because the rejection is no longer always Rust's own error
    // string: an unreadable IPC payload arrives as an Error, and `String` would
    // put its "Error: " prefix in front of the reason on the System page.
    appUpdateCheckError.value = errorDetail(e);
    console.warn('[app-update] update check failed; will retry next poll', e);
    return;
  }
  appUpdateCheckError.value = null;
  // The packaged updater is the authoritative source of "latest available" for a
  // packaged client. The engine's `/health` field cannot be: it is read from a
  // repo checkout, so every packaged install reports `'unknown'` (which
  // connection.ts now discards). Clear only what we ourselves set — see
  // {@link updaterOwnsLatestVersion}.
  //
  // The notes travel WITH the version, written and cleared on the same branches,
  // so the two can never end up describing different releases: a stale note
  // beside a fresh version would tell the user what a different update contains.
  if (offer) {
    latestTauriAppVersion.value = offer.version;
    latestTauriAppNotes.value = offer.notes;
    updaterOwnsLatestVersion = true;
  } else if (updaterOwnsLatestVersion) {
    latestTauriAppVersion.value = null;
    latestTauriAppNotes.value = null;
    updaterOwnsLatestVersion = false;
  }
  if (!offer) return;
  offerAppUpdate(offer.version);
}

/** Re-check for a packaged update because the user came BACK to the client
 *  (window focus / `visibilitychange` / `pageshow`), throttled to at most one
 *  network round-trip per {@link APP_UPDATE_RESUME_MIN_INTERVAL_MS}.
 *
 *  Every other update surface already reconciles on resume (the service worker,
 *  the frontend `BUILD_ID`, the engine build state, the unread set); the packaged
 *  updater was the one that did not, so a client left running past a release kept
 *  reporting itself current until the interval came round. That is what stranded a
 *  0.18.0 client on 2026-07-31 for six hours with 0.18.2 already published.
 *
 *  Deliberately does NOT restart the interval: the two are independent safety
 *  nets, and rescheduling on every window switch would let a busy user's timer
 *  never fire at all. */
export async function recheckAppUpdateOnResume(): Promise<void> {
  if (!isTauri()) return;
  if (
    lastCheckStartedAt !== null &&
    Date.now() - lastCheckStartedAt < APP_UPDATE_RESUME_MIN_INTERVAL_MS
  ) {
    return;
  }
  await checkForAppUpdate();
}

/** The newer packaged version available to install, or `null`. Single derivation
 *  of "is there an update?" — the notice, the button label, and the button's
 *  action all ask this one question, so they cannot drift apart. Reads the signal
 *  at call time, so calling it during render subscribes the caller normally. */
export function packagedUpdateVersion(): string | null {
  const latest = latestTauriAppVersion.value;
  const current = window.__LUCIDOS_APP_VERSION__;
  return latest && current && isNewerVersion(latest, current) ? latest : null;
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
  handleAppUpdateProgress({ version: latestTauriAppVersion.value, phase: 'checking' });
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

/** Start the periodic packaged app-update check (immediately + every
 *  {@link APP_UPDATE_POLL_MS}) and subscribe to the updater's progress stream.
 *  Tauri-only.
 *
 *  The immediate check runs on EVERY call, not just the first. The client process
 *  is long-resident while the workspace app remounts (reload, workspace switch,
 *  restart-reconnect), and the previous "return early if the timer exists" guard
 *  meant only the very first mount of a process ever checked. With an hours-long
 *  interval behind it, an update published mid-session stayed invisible until the
 *  app was fully quit. Only the TIMER and the SUBSCRIPTION are idempotent, so
 *  remounting cannot stack intervals or listeners.
 *
 *  The third net is {@link recheckAppUpdateOnResume}, wired to window focus in
 *  `hooks/useStartup.ts`: a client that neither remounts nor waits out the
 *  interval still notices a release the moment the user comes back to it. */
export function startAppUpdateChecks(): void {
  if (!isTauri()) return;
  void subscribeToAppUpdateProgress();
  void checkForAppUpdate();
  if (pollTimer !== null) return;
  pollTimer = setInterval(() => { void checkForAppUpdate(); }, APP_UPDATE_POLL_MS);
}

/** Stop the periodic check and drop the progress subscription (startup cleanup). */
export function stopAppUpdateChecks(): void {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
  if (unlistenProgress) {
    unlistenProgress();
    unlistenProgress = null;
  }
}
