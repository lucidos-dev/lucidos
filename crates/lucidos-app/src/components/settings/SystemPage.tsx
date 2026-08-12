import { useCallback, useEffect } from 'preact/hooks';
import {
  appUpdateCheckError,
  appUpdateProgress,
  connectionStatus,
  enginePackaged,
  engineStartedAt,
  engineVersion,
  latestEngineVersion,
  latestTauriAppNotes,
  latestTauriAppVersion,
  lucidosRelease,
  lucidosReleaseDirty,
  restartRequired,
  serviceWorkerBuildId,
  showToast,
  SETTINGS_SYSTEM_SUBPANEL_ITEMS,
  type SettingsNavKey,
  updateAvailable,
  visibleWorkspaceName,
  workspacePath,
} from '../../store/store';
import { confirmAndRestartEngine } from '../../store/actions/chat-changes';
import { appUpdateNarration, checkForAppUpdate, installAppUpdate, packagedUpdateVersion } from '../../store/actions/app-update';
import { cancelAppUpdate } from '../../utils/tauri';
import { openSettingsSubview } from '../../store/actions/menu';
import { requestServiceWorkerBuildId, refreshClient } from '../../hooks/sw-update';
import { formatBuildId } from '../../utils/buildId';
import { clientVersionLabel } from '../../utils/clientVersion';
import { isNewerVersion } from '../../utils/version';
import { formatShortTime } from '../../utils/formatTime';
import { BackupSection } from './BackupSection';
import { DiskUsagePage } from './DiskUsagePage';
import { MemoryInspector } from './MemoryInspector';
import { EnvironmentVariablesPage } from './EnvironmentVariablesPage';
import { DebuggingSection } from './DebuggingSection';
import { WhatsNewPage } from './WhatsNewPage';
import { restartControlHome } from './restartControl';
import { ThreadQueueView } from '../thread-queue/ThreadQueueView';

/** The SPA origin, read lazily so importing this module never touches the DOM. */
function getApiUrl(): string {
  return typeof window !== 'undefined' && window.location ? window.location.origin : '';
}

export type SystemPanel = 'overview' | 'whats-new' | 'thread-queue' | 'backup' | 'memory' | 'disk-usage' | 'environment-variables' | 'debugging';

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
  const name = visibleWorkspaceName.value;
  const path = workspacePath.value;
  const startedAt = engineStartedAt.value;
  const release = lucidosRelease.value;
  const releaseDirty = lucidosReleaseDirty.value;
  const engineVer = engineVersion.value;
  const latestEngineVer = latestEngineVersion.value;
  const latestTauriVer = latestTauriAppVersion.value;
  const restart = restartRequired.value;
  const update = updateAvailable.value;
  // Overview owns the restart control only in dev, where it genuinely rebuilds.
  // On a packaged install it lives in System > Debugging instead, labelled
  // "Restart Engine" (see restartControl.ts). Exactly one of the two renders.
  //
  // Dropping the button here does NOT strand the `restart` notice below, which
  // says "restart to activate": `restart` cannot be true while packaged. Both
  // routes that set it are dead there. The outdated-release check compares
  // against `latest_engine_version`, which the engine derives from the repo root
  // and so reports as the string `'unknown'` on a packaged install, and
  // `isNewerVersion` coerces that to NaN and returns false. And an applied
  // restart-requiring change needs a Lucidos-source coding-agent thread, which
  // `packaged` (engine-side `!has_lucidos_source()`, resolved from the running
  // binary's own path) is precisely the absence of. If packaged ever starts
  // reporting a real engine version, this notice needs to point at Debugging.
  const ownsRestart = restartControlHome(enginePackaged.value) === 'overview';
  const swBuildId = serviceWorkerBuildId.value;
  const swBuildLabel = swBuildId ? formatBuildId(swBuildId) : null;

  useEffect(() => {
    requestServiceWorkerBuildId();
  }, []);

  const handleRefresh = useCallback(() => {
    refreshClient();
  }, []);

  /** Update now if one is known, otherwise re-check on demand. The on-demand
   *  path matters because the automatic checks are periodic (an hourly poll, plus
   *  mount and window-resume rechecks): without it the only way to ask "is there
   *  something newer?" right now was to quit and relaunch. */
  const handleAppUpdate = useCallback(async () => {
    if (packagedUpdateVersion()) {
      await installAppUpdate();
      return;
    }
    await checkForAppUpdate();
    // Runs on USER intent, so unlike the background poll this reports both
    // outcomes rather than staying silent.
    if (appUpdateCheckError.value) {
      showToast(`Couldn't check for updates: ${appUpdateCheckError.value}`, 'error');
    } else if (!latestTauriAppVersion.value) {
      showToast('Lucidos is up to date', 'success');
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
  // Shared with the Lucidos menu's identity row, which names the same thing.
  const clientVersion = clientVersionLabel();
  // One derivation, shared with the button's action so the label and what the
  // click does can never disagree.
  const tauriHasUpdate = !!packagedUpdateVersion();
  // An update in flight OWNS this control: the same derivation the progress toast
  // renders, so the persistent surface and the transient one can never disagree
  // about what the update is doing. Terminal frames clear the signal, so this is
  // non-null exactly while a run is live.
  const updateRun = appUpdateProgress.value;
  const updateNarration = updateRun ? appUpdateNarration(updateRun) : null;
  const clientBehind = tauriClientVersion ? tauriHasUpdate : update;
  const clientBehindLabel = tauriClientVersion ? ` (latest: ${latestTauriVer})` : ' (update available)';

  function renderPanel() {
    switch (panel) {
      case 'whats-new': return <WhatsNewPage />;
      case 'thread-queue': return <ThreadQueueView />;
      case 'backup': return <BackupSection />;
      case 'memory': return <MemoryInspector />;
      case 'disk-usage': return <DiskUsagePage />;
      case 'environment-variables': return <EnvironmentVariablesPage />;
      case 'debugging': return <DebuggingSection />;
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
                <span class="system-info-value system-info-long">{path}</span>
              </div>
            )}
            <div class="system-info-row">
              <span class="system-info-label">API</span>
              <button class="system-info-value system-info-long system-api-url accent-link" onClick={copyApiUrl}>
                {getApiUrl()}
              </button>
            </div>
          </div>
        </div>

        {/* Locale (language + timezone) and the coding-agent binary paths used
            to render here. Neither is a system concern: Locale is a plain user
            preference and now has its own top-level category, and the binary
            paths moved to Coding Agents beside the repositories they run in.
            What is left is what System actually is: this workspace's
            connection, what version is running, and how to move it forward. */}
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
                {/* The build the controlling service worker was installed from —
                    normally identical to Client above, and different exactly
                    while a swap is mid-flight or the worker is wedged. Labelled
                    distinctly so the two build ids can't be read as one. */}
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
          {/* The PERSISTENT route to a packaged update. The in-app toast is
              transient and dismissable, so it cannot be the only way to reach
              one. Dismissing it used to strand the user until the next poll
              (or a full quit-and-relaunch). Once a run starts, the same live
              phase the toast narrates replaces the offer here. */}
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
              Lucidos {latestTauriVer} is available - update and restart to install
              {/* What is in it, before deciding to take it. The notes come from
                  the update manifest, so What's New shows THAT release above the
                  installed history; rendered only when the manifest carried
                  them, so the link never opens onto nothing. */}
              {latestTauriAppNotes.value && (
                <>
                  {' '}
                  <button
                    class="accent-link"
                    onClick={() => openSettingsSubview('whats-new')}
                  >
                    What's new
                  </button>
                </>
              )}
            </div>
          )}
          {/* A check that FAILED must not look like "you are up to date". */}
          {appUpdateCheckError.value && (
            <div class="system-notice">
              Couldn't check for updates: {appUpdateCheckError.value}
            </div>
          )}
          <div class="system-actions">
            <button class="action-btn" onClick={handleRefresh}>
              Refresh Client
            </button>
            {/* While a run is live this button must not offer to start another —
                it reports the phase instead, and turns into the same Cancel the
                toast offers for exactly as long as one is honest. */}
            {tauriClientVersion && (updateNarration
              ? (updateNarration.cancellable
                  ? <button class="action-btn action-btn-danger" onClick={() => { void cancelAppUpdate(); }}>Cancel Update</button>
                  : <button class="action-btn" disabled>Updating…</button>)
              : (
                <button class="action-btn" onClick={handleAppUpdate}>
                  {tauriHasUpdate ? 'Update & Restart' : 'Check for Updates'}
                </button>
              ))}
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

  return (
    <>
      <SystemPanelSwitcher activePanel={panel} />
      {renderPanel()}
    </>
  );
}
