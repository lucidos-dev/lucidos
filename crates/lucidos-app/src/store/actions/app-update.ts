import { showToast } from '../store';
import { isTauri } from '../../utils/platform';
import { checkAppUpdate, installAppUpdateAndRestart } from '../../utils/tauri';

/** How often the packaged client re-checks for an app update. The client is
 *  long-resident (the window can be closed while it stays alive in the menu bar),
 *  so a launch-only check would miss an update published mid-session. */
const APP_UPDATE_POLL_MS = 6 * 60 * 60 * 1000; // 6h

let pollTimer: ReturnType<typeof setInterval> | null = null;
let installing = false;

/** Check for a newer packaged build and, if one exists, surface the in-app
 *  "Update & restart" toast inside the workspace. Tauri-only — a plain browser /
 *  mobile PWA / dev build can't update the desktop app, so this is a no-op there.
 *
 *  Best-effort telemetry (frontend.md carve-out): it runs on a timer without user
 *  intent, so a failed check is logged, NOT toasted — the next poll retries, and
 *  the real user-facing failure surface is the install action's own toast (which
 *  DOES run on user intent). */
export async function checkForAppUpdate(): Promise<void> {
  if (!isTauri()) return;
  let version: string | null;
  try {
    version = await checkAppUpdate();
  } catch (e) {
    // Best-effort: next poll retries; install-time errors surface to the user.
    console.warn('[app-update] update check failed; will retry next poll', e);
    return;
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

/** Install the available packaged update and restart the whole stack. This runs on
 *  USER intent (the toast action), so a failure is surfaced as an error toast. On
 *  success the client re-execs and this page is torn down — no further code runs. */
async function installAppUpdate(): Promise<void> {
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
 *  {@link APP_UPDATE_POLL_MS}). Tauri-only; idempotent. */
export function startAppUpdateChecks(): void {
  if (!isTauri()) return;
  if (pollTimer !== null) return;
  void checkForAppUpdate();
  pollTimer = setInterval(() => { void checkForAppUpdate(); }, APP_UPDATE_POLL_MS);
}

/** Stop the periodic check (startup cleanup). */
export function stopAppUpdateChecks(): void {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}
