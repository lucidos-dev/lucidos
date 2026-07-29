import { showToast, latestTauriAppVersion, appUpdateCheckError } from '../store';
import { isTauri } from '../../utils/platform';
import { isNewerVersion } from '../../utils/version';
import { checkAppUpdate, installAppUpdateAndRestart } from '../../utils/tauri';

/** How often the packaged client re-checks for an app update. The client is
 *  long-resident (the window can be closed while it stays alive in the menu bar),
 *  so a launch-only check would miss an update published mid-session. */
const APP_UPDATE_POLL_MS = 6 * 60 * 60 * 1000; // 6h

let pollTimer: ReturnType<typeof setInterval> | null = null;
let installing = false;
/** Whether {@link latestTauriAppVersion} currently holds a value THIS module
 *  wrote. Gates the clear-on-no-update path so the updater never wipes a value
 *  it did not put there: in a Tauri DEV client `check_app_update` is a no-op
 *  returning null, which is indistinguishable from "up to date" — and blindly
 *  assigning that null would clobber the version `connection.ts` reads from the
 *  engine's `/health`, which is the only source dev has. The two would then
 *  fight on every poll. */
let updaterOwnsLatestVersion = false;

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
  let version: string | null;
  try {
    version = await checkAppUpdate();
  } catch (e) {
    // Recorded for Settings → System; the next poll retries.
    appUpdateCheckError.value = String(e);
    console.warn('[app-update] update check failed; will retry next poll', e);
    return;
  }
  appUpdateCheckError.value = null;
  // The packaged updater is the authoritative source of "latest available" for a
  // packaged client. The engine's `/health` field cannot be: it is read from a
  // repo checkout, so every packaged install reports `'unknown'` (which
  // connection.ts now discards). Clear only what we ourselves set — see
  // {@link updaterOwnsLatestVersion}.
  if (version) {
    latestTauriAppVersion.value = version;
    updaterOwnsLatestVersion = true;
  } else if (updaterOwnsLatestVersion) {
    latestTauriAppVersion.value = null;
    updaterOwnsLatestVersion = false;
  }
  if (!version) return;
  showToast(`Lucidos ${version} available`, 'info', {
    key: 'app-update-available',
    action: {
      label: 'Update & restart',
      onClick: () => { void installAppUpdate(); },
    },
  });
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
 *  USER intent (the toast action), so a failure is surfaced as an error toast. On
 *  success the client re-execs and this page is torn down — no further code runs. */
export async function installAppUpdate(): Promise<void> {
  if (installing) return;
  installing = true;
  try {
    await installAppUpdateAndRestart();
    // Reached only if the command resolved WITHOUT restarting (it normally never
    // returns) — allow a retry.
    installing = false;
  } catch (e) {
    installing = false;
    showToast(`Update failed: ${String(e)}`, 'error');
  }
}

/** Start the periodic packaged app-update check (immediately + every
 *  {@link APP_UPDATE_POLL_MS}). Tauri-only.
 *
 *  The immediate check runs on EVERY call, not just the first. The client process
 *  is long-resident while the workspace app remounts (reload, workspace switch,
 *  restart-reconnect), and the previous "return early if the timer exists" guard
 *  meant only the very first mount of a process ever checked — with a 6h interval
 *  behind it, an update published mid-session stayed invisible until the app was
 *  fully quit. Only the TIMER is idempotent, so remounting cannot stack intervals. */
export function startAppUpdateChecks(): void {
  if (!isTauri()) return;
  void checkForAppUpdate();
  if (pollTimer !== null) return;
  pollTimer = setInterval(() => { void checkForAppUpdate(); }, APP_UPDATE_POLL_MS);
}

/** Stop the periodic check (startup cleanup). */
export function stopAppUpdateChecks(): void {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}
