import { useCallback, useEffect } from 'preact/hooks';
import {
  connectionStatus,
  engineStartedAt,
  engineVersion,
  latestEngineVersion,
  latestTauriAppVersion,
  lucidosRelease,
  lucidosReleaseDirty,
  restartGroups,
  restartRequired,
  serviceWorkerBuildId,
  showConfirm,
  showToast,
  SETTINGS_SYSTEM_SUBPANEL_ITEMS,
  type SettingsNavKey,
  updateAvailable,
  workspaceName,
  workspacePath,
} from '../../store/store';
import { initiateEngineRestart } from '../../store/actions/chat-changes';
import { openSettingsSubview } from '../../store/actions/menu';
import { requestServiceWorkerBuildId, refreshClient } from '../../hooks/sw-update';
import { isNewerVersion } from '../../utils/version';
import { formatShortTime } from '../../utils/formatTime';
import { isTauri } from '../../utils/platform';
import { invoke } from '../../utils/tauri';
import { ENGINE_VERSION } from 'virtual:engine-version';
import { BackupSection } from './BackupSection';
import { DiskUsagePage } from './DiskUsagePage';
import { MemoryInspector } from './MemoryInspector';

/** The SPA origin, read lazily so importing this module never touches the DOM. */
function getApiUrl(): string {
  return typeof window !== 'undefined' && window.location ? window.location.origin : '';
}

export type SystemPanel = 'overview' | 'backup' | 'memory' | 'disk-usage';

const SYSTEM_PANELS: Array<{ key: SystemPanel; label: string; subview: SettingsNavKey }> = [
  { key: 'overview', label: 'Overview', subview: 'system' },
  ...SETTINGS_SYSTEM_SUBPANEL_ITEMS.map(item => ({
    key: item.key as Exclude<SystemPanel, 'overview'>,
    label: item.label,
    subview: item.key,
  })),
];

function SystemPanelSwitcher({ activePanel }: { activePanel: SystemPanel }) {
  return (
    <div class="settings-section system-subpanel-switcher">
      <div class="settings-row-options system-subpanel-options">
        {SYSTEM_PANELS.map(item => (
          <button
            key={item.key}
            class={`settings-option${activePanel === item.key ? ' active' : ''}`}
            aria-current={activePanel === item.key ? 'page' : undefined}
            onClick={() => openSettingsSubview(item.subview)}
          >
            {item.label}
          </button>
        ))}
      </div>
    </div>
  );
}

export function SystemPage({ panel = 'overview' }: { panel?: SystemPanel }) {
  const status = connectionStatus.value;
  const connected = status === 'connected';
  const name = workspaceName.value;
  const path = workspacePath.value;
  const startedAt = engineStartedAt.value;
  const release = lucidosRelease.value;
  const releaseDirty = lucidosReleaseDirty.value;
  const engineVer = engineVersion.value;
  const latestEngineVer = latestEngineVersion.value;
  const latestTauriVer = latestTauriAppVersion.value;
  const restart = restartRequired.value;
  const update = updateAvailable.value;
  const swBuildId = serviceWorkerBuildId.value;
  const buildLabel = swBuildId
    ? (swBuildId.startsWith('__') ? 'dev' : swBuildId)
    : null;

  useEffect(() => {
    requestServiceWorkerBuildId();
  }, []);

  const handleRefresh = useCallback(() => {
    refreshClient();
  }, []);

  const handleRestart = useCallback(async () => {
    const extraAction = isTauri()
      ? {
          label: 'Restart App',
          onClick: () => {
            invoke('restart_app').catch((e: unknown) => {
              showToast(`Failed to restart app: ${e}`, 'error');
            });
          },
        }
      : undefined;
    const groups = restartGroups.value;
    const details = groups.length > 0
      ? {
          intro: 'These changes will be applied:',
          groups: groups.map(g => ({ header: g.threadTitle, items: g.commits })),
        }
      : undefined;
    if (await showConfirm('Restart engine?', 'Restart', { extraAction, variant: 'default', details })) {
      await initiateEngineRestart();
    }
  }, []);

  const copyApiUrl = useCallback(() => {
    navigator.clipboard.writeText(getApiUrl()).then(
      () => showToast('Copied to clipboard', 'success'),
      () => showToast('Failed to copy', 'error'),
    );
  }, []);

  const hasEngineUpdate = engineVer && latestEngineVer && isNewerVersion(latestEngineVer, engineVer);
  const tauriClientVersion = window.__LUCIDOS_APP_VERSION__;
  const clientVersion = tauriClientVersion ?? ENGINE_VERSION;
  const tauriHasUpdate = !!(tauriClientVersion && latestTauriVer && isNewerVersion(latestTauriVer, tauriClientVersion));
  const clientBehind = tauriClientVersion ? tauriHasUpdate : update;
  const clientBehindLabel = tauriClientVersion ? ` (latest: ${latestTauriVer})` : ' (update available)';

  function renderPanel() {
    switch (panel) {
      case 'backup': return <BackupSection />;
      case 'memory': return <MemoryInspector />;
      case 'disk-usage': return <DiskUsagePage />;
      default: return renderOverview();
    }
  }

  function renderOverview() {
    return (
      <>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="system:connection">Connection</div>
          <div class="system-status-row">
            <span class={`status-dot ${status}`} />
            <span>{status === 'connecting' ? 'Connecting...' : connected ? 'Connected' : 'Disconnected'}</span>
          </div>
          <div class="system-info-list">
            <div class="system-info-row">
              <span class="system-info-label">Workspace</span>
              <span class="system-info-value">{name || path || 'unknown'}</span>
            </div>
            {path && name && (
              <div class="system-info-row">
                <span class="system-info-label">Path</span>
                <span class="system-info-value system-info-path">{path}</span>
              </div>
            )}
            <div class="system-info-row">
              <span class="system-info-label">API</span>
              <button class="system-info-value system-api-url accent-link" onClick={copyApiUrl}>
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
            {buildLabel && (
              <div class="system-info-row">
                <span class="system-info-label">Build</span>
                <span class="system-info-value">{buildLabel}</span>
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
              <span class="system-update">*</span> code has changed since the {release} release
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
            <button class="action-btn" onClick={handleRestart}>
              Rebuild &amp; Restart
            </button>
          </div>
        </div>
      </>
    );
  }

  return (
    <>
      <SystemPanelSwitcher activePanel={panel} />
      {renderPanel()}
    </>
  );
}
