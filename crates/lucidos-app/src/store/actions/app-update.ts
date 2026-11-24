import { setAppUpdateProgressHandler } from './appUpdateProgressEvents';
export { startAppUpdateProgress, stopAppUpdateProgress } from './appUpdateProgressEvents';
import { showToast, removeToast, clientOfferedRelease, latestTauriAppNotes, appUpdateCheckError, appUpdateCheckInFlight, appUpdateProgress, lucidosRelease, relayedUpdate, releaseCheck, settingsScrollTarget } from '../store';
import { isNewerVersion } from '../../utils/version';
import type { ToastAction } from '../types';
import { packagedUpdateVersion } from '../packagedUpdate';
import { relayAvailable, relayClientReady } from '../updateRelay';
import { refreshUpdateRelay, resumeRelayedUpdate, watchRelayedUpdate, type RelayNews } from './update-relay';
import { openWhatsNew, openSettingsSubview } from './menu';
import { isTauri, thisDeviceIsMobile } from '../../utils/platform';
import { errorDetail } from '../../utils/errorDetail';
import { requestUpdateRelay, requestUpdateCheck } from '../../api/client/control';
import {
  checkAppUpdate,
  installAppUpdateAndRestart,
  type AppUpdateOffer,
  type AppUpdateProgress,
} from '../../utils/tauri';

const UPDATE_TOAST_KEY = 'app-update-available';

const NO_CHECK_HERE = 'this install has no update check';

const UPDATE_ON_DESKTOP = 'Update Lucidos on the machine that runs this workspace, not from this device.';
const UPDATE_ON_DESKTOP_KEY = 'app-update-on-desktop';

const RELAY_DID_NOT_RUN =
  'The desktop app did not run the update. Make sure Lucidos is open on that Mac, then try again.';

let installing = false;
let lastOfferedVersion: string | null = null;
let inFlightCheck: Promise<UpdateCheckVerdict> | null = null;

export type UpdateCheckVerdict =
  | { kind: 'available'; version: string }
  | { kind: 'up-to-date' }
  | { kind: 'failed'; reason: string }
  | { kind: 'installing' }
  | { kind: 'shadowed'; engine: string; client: string };

function offerAppUpdate(version: string): void {
  if (thisDeviceIsMobile()) return;
  const route = updateRoute(true);
  const follow = () => { void followUpdateRoute(route); };
  const action: ToastAction = route === 'install'
    ? { label: 'Update & restart', variant: 'confirm', onClick: follow }
    : route === 'relay'
      ? { label: 'Update desktop app', variant: 'confirm', onClick: follow }
      : { label: 'How to update', onClick: follow };
  showToast(`Lucidos ${version} available`, 'info', {
    key: UPDATE_TOAST_KEY,
    action,
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

function handleAppUpdateProgress(frame: AppUpdateProgress): void {
  if (frame.phase === 'cancelled') {
    appUpdateProgress.value = null;
    installing = false;
    const version = frame.version ?? packagedUpdateVersion();
    if (version) offerAppUpdate(version);
    else removeToast(UPDATE_TOAST_KEY);
    return;
  }
  if (frame.phase === 'failed') {
    appUpdateProgress.value = null;
    installing = false;
    showToast(`Update failed: ${frame.message}`, 'error', { key: UPDATE_TOAST_KEY });
    return;
  }
  if (frame.phase === 'bundle-swap-failed') {
    appUpdateProgress.value = null;
    installing = false;
    showToast(frame.message, 'error', { key: UPDATE_TOAST_KEY });
    return;
  }
  appUpdateProgress.value = frame;
  removeToast(UPDATE_TOAST_KEY);
}

export async function refreshReleaseCheck(force = false): Promise<UpdateCheckVerdict> {
  try {
    releaseCheck.value = await requestUpdateCheck(force);
  } catch (e) {
    console.warn('[app-update] gateway release check unavailable; retried on next resume', e);
    const reason = errorDetail(e);
    if (force) appUpdateCheckError.value = reason;
    return { kind: 'failed', reason };
  }
  if (!isTauri()) await refreshUpdateRelay();
  appUpdateCheckError.value = releaseCheck.value.last_error;
  const latest = releaseCheck.value.latest;
  latestTauriAppNotes.value = latest?.notes ?? null;
  if (!releaseCheck.value.supported) return { kind: 'failed', reason: NO_CHECK_HERE };
  if (!latest) {
    const lastError = releaseCheck.value.last_error;
    return lastError ? { kind: 'failed', reason: lastError } : { kind: 'up-to-date' };
  }
  if (appUpdateProgress.value) return { kind: 'installing' };
  if (lastOfferedVersion !== latest.version || force) {
    lastOfferedVersion = latest.version;
    offerAppUpdate(latest.version);
  }
  return { kind: 'available', version: latest.version };
}

export function checkForUpdatesNow(): Promise<UpdateCheckVerdict> {
  if (inFlightCheck) return inFlightCheck;
  appUpdateCheckInFlight.value = true;
  inFlightCheck = runUserCheck().finally(() => {
    inFlightCheck = null;
    appUpdateCheckInFlight.value = false;
  });
  return inFlightCheck;
}

export type UpdateRoute = 'install' | 'relay' | 'check' | 'guide' | 'desktop';

export function sessionCanInstall(): boolean {
  return isTauri() && releaseCheck.value?.latest?.install !== 'installer-rerun';
}

export function canCheckForUpdatesHere(): boolean {
  const check = releaseCheck.value;
  return check ? check.supported : isTauri();
}

export function updateRoute(
  hasNewerRelease: boolean = packagedUpdateVersion() !== null,
): UpdateRoute {
  if (hasNewerRelease && !sessionCanInstall() && relayAvailable(hasNewerRelease)) return 'relay';
  if (thisDeviceIsMobile()) {
    return !hasNewerRelease && relayClientReady() && canCheckForUpdatesHere() ? 'check' : 'desktop';
  }
  if (hasNewerRelease) return sessionCanInstall() ? 'install' : 'guide';
  return canCheckForUpdatesHere() ? 'check' : 'guide';
}

export function updateControlLabel(route: UpdateRoute, checking: boolean): string {
  if (checking) return 'Checking…';
  if (route === 'install') return 'Update & Restart';
  if (route === 'relay') return 'Update Desktop App';
  return route === 'check' ? 'Check for Updates' : 'How to Update';
}

export async function followUpdateRoute(route: UpdateRoute): Promise<void> {
  if (route === 'install') {
    await installAppUpdate();
    return;
  }
  if (route === 'relay') {
    await relayUpdateToDesktop();
    return;
  }
  if (route === 'check') {
    reportUpdateCheck(await checkForUpdatesNow());
    return;
  }
  if (route === 'desktop') {
    showToast(UPDATE_ON_DESKTOP, 'info', { key: UPDATE_ON_DESKTOP_KEY });
    return;
  }
  settingsScrollTarget.value = 'system:maintenance';
  openSettingsSubview('system-overview');
}

export function reportUpdateCheck(verdict: UpdateCheckVerdict): void {
  if (verdict.kind === 'up-to-date') showToast('Lucidos is up to date', 'success');
  else if (verdict.kind === 'failed') {
    showToast(`Couldn't check for updates: ${verdict.reason}`, 'error');
  } else if (verdict.kind === 'shadowed') {
    showToast(
      `This workspace is served by Lucidos ${verdict.engine}, but this app is ` +
        `${verdict.client}. See System > Overview.`,
      'error',
    );
  }
}

export function shadowedEngine(): { engine: string; client: string } | null {
  const client = typeof window !== 'undefined' ? window.__LUCIDOS_APP_RELEASE__ : undefined;
  const engine = lucidosRelease.value;
  if (!client || !engine) return null;
  return isNewerVersion(client, engine) ? { engine, client } : null;
}

async function runUserCheck(): Promise<UpdateCheckVerdict> {
  if (appUpdateProgress.value) return { kind: 'installing' };
  if (releaseCheck.value) return refreshReleaseCheck(true);
  const shadow = shadowedEngine();
  if (shadow) return { kind: 'shadowed', ...shadow };
  if (isTauri()) return checkAppUpdateViaClient();
  return { kind: 'failed', reason: 'this session has no update check to run' };
}

export async function checkAppUpdateViaClient(): Promise<UpdateCheckVerdict> {
  if (!isTauri()) return { kind: 'failed', reason: 'no client updater in this session' };
  if (appUpdateProgress.value) return { kind: 'installing' };
  let offer: AppUpdateOffer | null;
  try {
    offer = await checkAppUpdate();
  } catch (e) {
    const reason = errorDetail(e);
    appUpdateCheckError.value = reason;
    return { kind: 'failed', reason };
  }
  appUpdateCheckError.value = null;
  clientOfferedRelease.value = offer?.version ?? null;
  latestTauriAppNotes.value = offer?.notes ?? null;
  if (!offer) return { kind: 'up-to-date' };
  offerAppUpdate(offer.version);
  return { kind: 'available', version: offer.version };
}

export function canInstallUpdateHere(): boolean {
  return packagedUpdateVersion() !== null && sessionCanInstall();
}

export async function installAppUpdate(): Promise<void> {
  if (installing) return;
  installing = true;
  handleAppUpdateProgress({ version: packagedUpdateVersion(), phase: 'checking' });
  try {
    await installAppUpdateAndRestart();
    installing = false;
    if (appUpdateProgress.value) {
      appUpdateProgress.value = null;
      removeToast(UPDATE_TOAST_KEY);
    }
  } catch (e) {
    installing = false;
    if (appUpdateProgress.value) {
      appUpdateProgress.value = null;
      showToast(`Update failed: ${errorDetail(e)}`, 'error', { key: UPDATE_TOAST_KEY });
    }
  }
}


export async function relayUpdateToDesktop(): Promise<void> {
  if (relayedUpdate.value) return;
  const fromVersion = releaseCheck.value?.current_version;
  if (!fromVersion) {
    showToast("Couldn't update the desktop app: the running version is not known yet", 'error', {
      key: UPDATE_TOAST_KEY,
    });
    return;
  }
  let ticket;
  try {
    ticket = await requestUpdateRelay();
  } catch (e) {
    showToast(`Couldn't update the desktop app: ${errorDetail(e)}`, 'error', { key: UPDATE_TOAST_KEY });
    return;
  }
  handleAppUpdateProgress({ version: ticket.version, phase: 'checking' });
  watchRelayedUpdate(
    { id: ticket.id, version: ticket.version, fromVersion, since: Date.now() },
    handleRelayNews,
  );
}

export function resumeUpdateRelay(): void {
  resumeRelayedUpdate(handleRelayNews);
}

function handleRelayNews(news: RelayNews): void {
  if (news.kind === 'progress' || news.kind === 'ended') {
    handleAppUpdateProgress(news.frame);
    return;
  }
  appUpdateProgress.value = null;
  if (news.kind === 'succeeded') {
    showToast(`The desktop app is now on Lucidos ${news.version}`, 'success', { key: UPDATE_TOAST_KEY });
  } else {
    showToast(RELAY_DID_NOT_RUN, 'error', { key: UPDATE_TOAST_KEY });
  }
}

setAppUpdateProgressHandler(handleAppUpdateProgress);
