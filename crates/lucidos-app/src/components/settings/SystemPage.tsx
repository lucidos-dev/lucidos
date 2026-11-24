import { useCallback, useEffect } from 'preact/hooks';
import {
  appUpdateCheckError,
  appUpdateCheckInFlight,
  appUpdateProgress,
  connectionStatus,
  connectionNoticeStatus,
  enginePackaged,
  engineStartedAt,
  engineVersion,
  latestEngineVersion,
  latestTauriAppNotes,
  lucidosRelease,
  releaseCheck,
  lucidosReleaseDirty,
  restartRequired,
  serviceWorkerBuildId,
  showToast,
  updateAvailable,
  visibleWorkspaceName,
  relayedUpdate,
  workspacePath,
} from '../../store/store';
import { confirmAndRestartEngine } from '../../store/actions/chat-changes';
import {
  canCheckForUpdatesHere,
  canInstallUpdateHere,
  followUpdateRoute,
  refreshReleaseCheck,
  sessionCanInstall,
  shadowedEngine,
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
import { openWhatsNew } from '../../store/actions/menu';
import { requestServiceWorkerBuildId, refreshClient } from '../../hooks/sw-update';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { formatBuildId } from '../../utils/buildId';
import { clientVersionLabel } from '../../utils/clientVersion';
import { isNewerBuildId } from '../../utils/version';
import { copyToClipboard } from '../../utils/clipboard';
import { formatShortTime } from '../../utils/formatTime';
import { connectionNotice } from '../../utils/connectionNotice';
import { errorDetail } from '../../utils/errorDetail';
import { BackupSection } from './BackupSection';
import { SnapshotSection } from './SnapshotSection';
import { RetainedRestoreSources } from './RetainedRestoreSources';
import { InstallsSection } from './InstallsSection';
import { DiskUsagePage } from './DiskUsagePage';
import { PromptFootprintPage } from './PromptFootprintPage';
import { MemoryPage } from './MemoryPage';
import { EnvironmentVariablesPage } from './EnvironmentVariablesPage';
import { DebuggingSection } from './DebuggingSection';
import { CommunicationSurfacesPage } from './CommunicationSurfacesPage';
import { WhatsNewPage } from './WhatsNewPage';
import { ReleaseNoticesPage } from './ReleaseNoticesPage';
import { UpdateChannelNotices, UpdateChannelSetting } from './UpdateChannelSetting';
import { restartControlHome } from './restartControl';
import { Explainer } from '../shared/Explainer';
import { ThreadQueueView } from '../thread-queue/ThreadQueueView';

function getApiUrl(): string {
  return typeof window !== 'undefined' && window.location ? window.location.origin : '';
}

export type SystemPanel = 'overview' | 'release-notices' | 'whats-new' | 'thread-queue' | 'backup' | 'memory' | 'disk-usage' | 'prompt-footprint' | 'environment-variables' | 'debugging' | 'communication-surfaces';

function checkForUpdateNow(): Promise<unknown> {
  return refreshReleaseCheck(true);
}

export function SystemPage({ panel }: { panel: SystemPanel }) {
  usePanelRefresh('the update check', panel === 'overview' ? checkForUpdateNow : null);
  const status = connectionStatus.value;
  const name = visibleWorkspaceName.value;
  const noticeStatus = connectionNoticeStatus.value;
  const notice = connectionNotice(noticeStatus, name, 'full');
  const path = workspacePath.value;
  const startedAt = engineStartedAt.value;
  const release = lucidosRelease.value;
  const releaseDirty = lucidosReleaseDirty.value;
  const engineVer = engineVersion.value;
  const latestEngineVer = latestEngineVersion.value;
  const restart = restartRequired.value;
  const update = updateAvailable.value;
  const ownsRestart = restartControlHome(enginePackaged.value === true) === 'overview';
  const swBuildId = serviceWorkerBuildId.value;
  const swBuildLabel = swBuildId ? formatBuildId(swBuildId) : null;

  useEffect(() => {
    requestServiceWorkerBuildId();
  }, []);

  const handleRefresh = useCallback(() => {
    refreshClient();
  }, []);

  const setReleaseCheckEnabled = useCallback(async (enabled: boolean) => {
    try {
      releaseCheck.value = await setReleaseCheckConfig({ enabled });
    } catch (e) {
      showToast(`Couldn't save the update-check setting: ${errorDetail(e)}`, 'error');
    }
  }, []);


  const hasEngineUpdate = engineVer && latestEngineVer && isNewerBuildId(latestEngineVer, engineVer);
  const shadowed = shadowedEngine();
  const tauriClientVersion = window.__LUCIDOS_APP_VERSION__;
  const clientVersion = clientVersionLabel();
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
  const clientBehind = tauriClientVersion ? tauriHasUpdate : update;
  const clientBehindLabel = tauriClientVersion ? ` (latest: ${offeredVersion})` : ' (update available)';
  const guidance = updateGuidance({
    engineAnswered: status === 'connected',
    packaged: enginePackaged.value === true,
    hasOffer: tauriHasUpdate,
    sessionCanInstall: sessionCanInstall(),
    canCheckHere,
    install: offer?.install ?? null,
  });

  function renderPanel() {
    switch (panel) {
      case 'release-notices': return <ReleaseNoticesPage />;
      case 'whats-new': return <WhatsNewPage />;
      case 'thread-queue': return <ThreadQueueView />;
      case 'backup': return <><SnapshotSection /><BackupSection /><RetainedRestoreSources /></>;
      case 'memory': return <MemoryPage />;
      case 'disk-usage': return <DiskUsagePage />;
      case 'prompt-footprint': return <PromptFootprintPage />;
      case 'environment-variables': return <EnvironmentVariablesPage />;
      case 'debugging': return <DebuggingSection />;
      case 'communication-surfaces': return <CommunicationSurfacesPage />;
      default: return renderOverview();
    }
  }

  function renderOverview() {
    return (
      <>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="system:connection">Connection</div>
          <div class={`system-status-row${notice ? ' has-note' : ''}`}>
            <span class={`status-dot ${noticeStatus}`} aria-hidden="true" />
            <span>{notice ? notice.title : 'Connected'}</span>
          </div>
          {notice && <p class="system-status-note">{notice.detail}</p>}
          <div class="system-info-list">
            <div class="system-info-row">
              <span class="system-info-label">Workspace</span>
              <span class="system-info-value">{name || path || 'unknown'}</span>
            </div>
            {path && name && (
              <div class="system-info-row">
                <span class="system-info-label">Path</span>
                <span class="system-info-value system-info-long">{path}</span>
              </div>
            )}
            <div class="system-info-row">
              <span class="system-info-label">API</span>
              <button class="system-info-value system-info-long system-api-url accent-link" onClick={() => copyToClipboard(getApiUrl())}>
                {getApiUrl()}
              </button>
            </div>
          </div>
        </div>

        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="system:versions">Versions</div>
          <div class="system-info-list">
            {release && (
              <div class="system-info-row">
                <span class="system-info-label">Lucidos</span>
                <span class="system-info-value">
                  {release}
                  {releaseDirty && <span class="system-update"> *</span>}
                </span>
              </div>
            )}
            {engineVer && (
              <div class="system-info-row">
                <span class="system-info-label">Engine</span>
                <span class="system-info-value">
                  {engineVer}
                  {hasEngineUpdate && (
                    <span class="system-update"> (latest: {latestEngineVer})</span>
                  )}
                </span>
              </div>
            )}
            {clientVersion && (
              <div class="system-info-row">
                <span class="system-info-label">Client</span>
                <span class="system-info-value">
                  {clientVersion}
                  {clientBehind && (
                    <span class="system-update">{clientBehindLabel}</span>
                  )}
                </span>
              </div>
            )}
            {swBuildLabel && (
              <div class="system-info-row">
                <span class="system-info-label">Service worker</span>
                <span class="system-info-value">{swBuildLabel}</span>
              </div>
            )}
            {startedAt && (
              <div class="system-info-row">
                <span class="system-info-label">Uptime</span>
                <span class="system-info-value">since {formatShortTime(new Date(startedAt))}</span>
              </div>
            )}
          </div>
          {release && releaseDirty && (
            <div class="system-footnote">
              * code has changed since the {release} release
            </div>
          )}
          {shadowed && (
            <div class="system-notice">
              This workspace is served by Lucidos {shadowed.engine}, but this app is{' '}
              {shadowed.client}. Installs, below, lists what is on this machine.
            </div>
          )}
        </div>

        <InstallsSection />

        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="system:maintenance">Maintenance</div>
          <UpdateChannelNotices check={check} />
          {restart && (
            <div class="system-notice">
              {hasEngineUpdate
                ? 'New engine version available - restart to activate'
                : 'Engine changes applied - restart to activate'}
            </div>
          )}
          {update && !restart && (
            <div class="system-notice">
              Client update available - refresh to activate
            </div>
          )}
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
              downloaded. Pull the release, then Rebuild &amp; Restart.
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
          <div class="system-actions">
            <button class="action-btn" onClick={handleRefresh}>
              Refresh Client
            </button>
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
              : offersUpdateControl && (
                <button
                  class="action-btn"
                  onClick={() => { void followUpdateRoute(pageRoute); }}
                  disabled={checking}
                >
                  {updateControlLabel(pageRoute, checking)}
                </button>
              )}
            {ownsRestart && (
              <button class="action-btn" onClick={() => { void confirmAndRestartEngine(); }}>
                Rebuild &amp; Restart
              </button>
            )}
          </div>
          {check?.supported && (
            <div class="settings-row" data-search-anchor="system:update-check">
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
      </>
    );
  }

  return <div class="system-page">{renderPanel()}</div>;
}
