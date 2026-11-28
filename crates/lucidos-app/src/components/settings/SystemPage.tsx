import { useCallback, useEffect } from 'preact/hooks';
import {
  connectionNoticeStatus,
  enginePackaged,
  engineStartedAt,
  engineVersion,
  latestEngineVersion,
  lucidosRelease,
  lucidosReleaseDirty,
  restartRequired,
  serviceWorkerBuildId,
  updateAvailable,
  visibleWorkspaceName,
  workspacePath,
} from '../../store/store';
import { confirmAndRestartEngine } from '../../store/actions/chat-changes';
import { shadowedEngine } from '../../store/actions/app-update';
import { packagedUpdateVersion } from '../../store/packagedUpdate';
import { openSettingsSubview } from '../../store/actions/menu';
import { requestServiceWorkerBuildId, refreshClient } from '../../hooks/sw-update';
import { formatBuildId } from '../../utils/buildId';
import { clientVersionLabel } from '../../utils/clientVersion';
import { isNewerBuildId } from '../../utils/version';
import { copyToClipboard } from '../../utils/clipboard';
import { formatShortTime } from '../../utils/formatTime';
import { connectionNotice } from '../../utils/connectionNotice';
import { BackupSection } from './BackupSection';
import { SnapshotSection } from './SnapshotSection';
import { RetainedRestoreSources } from './RetainedRestoreSources';
import { DiskUsagePage } from './DiskUsagePage';
import { PromptFootprintPage } from './PromptFootprintPage';
import { MemoryPage } from './MemoryPage';
import { EnvironmentVariablesPage } from './EnvironmentVariablesPage';
import { DebuggingSection } from './DebuggingSection';
import { DeveloperSection } from './DeveloperSection';
import { CommunicationSurfacesPage } from './CommunicationSurfacesPage';
import { WhatsNewPage } from './WhatsNewPage';
import { ReleaseNoticesPage } from './ReleaseNoticesPage';
import { restartControlHome } from './restartControl';
import { ThreadQueueView } from '../thread-queue/ThreadQueueView';

function getApiUrl(): string {
  return typeof window !== 'undefined' && window.location ? window.location.origin : '';
}

export type SystemPanel = 'overview' | 'release-notices' | 'whats-new' | 'thread-queue' | 'backup' | 'memory' | 'disk-usage' | 'prompt-footprint' | 'environment-variables' | 'debugging' | 'developer' | 'communication-surfaces';

export function SystemPage({ panel }: { panel: SystemPanel }) {
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

  const hasEngineUpdate = engineVer && latestEngineVer && isNewerBuildId(latestEngineVer, engineVer);
  const shadowed = shadowedEngine();
  const tauriClientVersion = window.__LUCIDOS_APP_VERSION__;
  const clientVersion = clientVersionLabel();
  const offeredVersion = packagedUpdateVersion();
  const tauriHasUpdate = !!offeredVersion;
  const clientBehind = tauriClientVersion ? tauriHasUpdate : update;
  const clientBehindLabel = tauriClientVersion ? ` (latest: ${offeredVersion})` : ' (update available)';

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
      case 'developer': return <DeveloperSection />;
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
              {shadowed.client}.{' '}
              <button class="accent-link" onClick={() => openSettingsSubview('updates')}>
                Updates &amp; Installs
              </button>{' '}
              lists what is on this machine.
            </div>
          )}
        </div>

        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="system:maintenance">Maintenance</div>
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
          <div class="system-actions">
            <button class="action-btn" onClick={handleRefresh}>
              Refresh Client
            </button>
            {ownsRestart && (
              <button class="action-btn" onClick={() => { void confirmAndRestartEngine(); }}>
                Rebuild &amp; Restart
              </button>
            )}
          </div>
        </div>
      </>
    );
  }

  return <div class="system-page">{renderPanel()}</div>;
}
