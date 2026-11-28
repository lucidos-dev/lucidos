import { useCallback } from 'preact/hooks';
import {
  appUpdateCheckError,
  appUpdateCheckInFlight,
  appUpdateProgress,
  connectionStatus,
  enginePackaged,
  latestTauriAppNotes,
  releaseCheck,
  relayedUpdate,
  showToast,
} from '../../store/store';
import {
  canCheckForUpdatesHere,
  canInstallUpdateHere,
  followUpdateRoute,
  refreshReleaseCheck,
  sessionCanInstall,
  updateControlLabel,
  updateRoute,
  type UpdateRoute,
} from '../../store/actions/app-update';
import { updateGuidance } from './updateGuidance';
import { packagedUpdateVersion } from '../../store/packagedUpdate';
import { thisDeviceIsMobile } from '../../utils/platform';
import { setReleaseCheckConfig } from '../../api/client/control';
import { appUpdateNarration } from '../../store/progressDialogCopy';
import { cancelAppUpdate } from '../../utils/tauri';
import { openMaintenanceSettings, openWhatsNew } from '../../store/actions/menu';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { copyToClipboard } from '../../utils/clipboard';
import { errorDetail } from '../../utils/errorDetail';
import { InstallsSection } from './InstallsSection';
import { EngineBuildsSection } from './EngineBuildsSection';
import { UpdateChannelNotices, UpdateChannelSetting } from './UpdateChannelSetting';
import { Explainer } from '../shared/Explainer';

function checkForUpdateNow(): Promise<unknown> {
  return refreshReleaseCheck(true);
}

export function UpdatesPage() {
  usePanelRefresh('the update check', checkForUpdateNow);
  const status = connectionStatus.value;

  const setReleaseCheckEnabled = useCallback(async (enabled: boolean) => {
    try {
      releaseCheck.value = await setReleaseCheckConfig({ enabled });
    } catch (e) {
      showToast(`Couldn't save the update-check setting: ${errorDetail(e)}`, 'error');
    }
  }, []);

  const offeredVersion = packagedUpdateVersion();
  const canInstallHere = canInstallUpdateHere();
  const canCheckHere = canCheckForUpdatesHere();
  const route = updateRoute();
  const relay = route === 'relay';
  const phoneChecksViaDesktop = route === 'check' && thisDeviceIsMobile();
  const offersUpdateControl =
    relay || phoneChecksViaDesktop || ((canInstallHere || canCheckHere) && !thisDeviceIsMobile());
  const pageRoute: UpdateRoute = relay ? 'relay' : canInstallHere ? 'install' : 'check';
  const tauriHasUpdate = !!offeredVersion;
  const check = releaseCheck.value;
  const offer = check?.latest ?? null;
  const updateCommand = offer?.install === 'installer-rerun' ? offer.command : null;
  const updateRun = appUpdateProgress.value;
  const updateNarration = updateRun ? appUpdateNarration(updateRun) : null;
  const checking = appUpdateCheckInFlight.value;
  const guidance = updateGuidance({
    engineAnswered: status === 'connected',
    packaged: enginePackaged.value === true,
    hasOffer: tauriHasUpdate,
    sessionCanInstall: sessionCanInstall(),
    canCheckHere,
    install: offer?.install ?? null,
  });

  return (
    <div class="system-page">
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="updates:updates">Updates</div>
        <UpdateChannelNotices check={check} />
        {updateNarration ? (
          <div class="system-notice">
            {updateNarration.message}
            {updateNarration.progress !== null && (
              <div class="progress-bar system-update-progress">
                <div class="progress-bar-fill" style={`width: ${Math.round(updateNarration.progress * 100)}%`} />
              </div>
            )}
          </div>
        ) : tauriHasUpdate && (
          <div class="system-notice">
            {canInstallHere
              ? `Lucidos ${offeredVersion} is available - update and restart to install`
              : `Lucidos ${offeredVersion} is available`}
            {latestTauriAppNotes.value && (
              <>
                {' '}
                <button
                  class="accent-link"
                  onClick={() => openWhatsNew(offeredVersion)}
                >
                  What's new
                </button>
              </>
            )}
          </div>
        )}
        {updateCommand && (
          <div class="system-notice">
            Re-run the installer to update:
            {' '}
            <code>{updateCommand}</code>
            {' '}
            <button class="accent-link" onClick={() => copyToClipboard(updateCommand)}>
              Copy
            </button>
          </div>
        )}
        {guidance === 'source-checkout' && (
          <div class="system-notice system-guidance">
            This engine runs from a source checkout, so no update is
            downloaded. Pull the release, then use Rebuild &amp; Restart
            under
            {' '}
            <button class="accent-link" onClick={openMaintenanceSettings}>
              System → Overview → Maintenance
            </button>
            .
          </div>
        )}
        {guidance === 'install-in-the-app' && (
          <div class="system-notice system-guidance">
            An update installs from the Lucidos app on the machine that runs
            this workspace, never from a browser session.
          </div>
        )}
        {appUpdateCheckError.value && (
          <div class="system-notice">
            Couldn't check for updates: {appUpdateCheckError.value}
          </div>
        )}
        {(updateNarration || offersUpdateControl) && (
          <div class="system-actions">
            {updateNarration
              ? (updateNarration.cancellable && !relayedUpdate.value
                  ? <button
                      class="action-btn action-btn-danger"
                      onClick={() => {
                        cancelAppUpdate().catch((e) =>
                          showToast(`Could not cancel the update: ${errorDetail(e)}`, 'error'));
                      }}
                    >Cancel Update</button>
                  : <button class="action-btn" disabled>Updating…</button>)
              : (
                <button
                  class="action-btn"
                  onClick={() => { void followUpdateRoute(pageRoute); }}
                  disabled={checking}
                >
                  {updateControlLabel(pageRoute, checking)}
                </button>
              )}
          </div>
        )}
        {check?.supported && (
          <div class="settings-row" data-search-anchor="updates:update-check">
            <span class="settings-row-label">
              Check for updates automatically
              <Explainer title="Check for updates automatically">
                <p>
                  While this is on, Lucidos asks <code>lucidos.dev</code> once an
                  hour whether a newer version is published. It is a regular web
                  request, and it says which platform, architecture and version
                  you run.
                </p>
                <p>
                  One request per machine, from the gateway, however many windows
                  you have open. A gateway started from a source checkout never
                  asks at all.
                </p>
                <p>
                  Nothing else is sent, nothing identifies you, and nothing
                  installs itself. Taking an update is always your click.
                </p>
                <p>
                  Turn it off and Lucidos stops asking on its own. You can still
                  check by hand with the button above.
                </p>
              </Explainer>
            </span>
            <label class="toggle-switch">
              <input
                type="checkbox"
                checked={check.enabled}
                onChange={(e) => {
                  const on = (e.currentTarget as HTMLInputElement).checked;
                  void setReleaseCheckEnabled(on);
                }}
              />
              <span class="toggle-slider" />
            </label>
          </div>
        )}
        {check && <UpdateChannelSetting check={check} />}
      </div>

      <EngineBuildsSection />

      <InstallsSection />
    </div>
  );
}
