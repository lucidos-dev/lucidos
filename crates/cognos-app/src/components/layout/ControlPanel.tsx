import { useState, useRef, useEffect, useCallback } from 'preact/hooks';
import { signal } from '@preact/signals';
import { connectionStatus, workspaceName, workspacePath, engineStartedAt, lucidosRelease, engineVersion, latestEngineVersion, latestTauriAppVersion, restartRequired, updateAvailable, restartGroups, showConfirm, showToast } from '../../store/store';
import { isNewerVersion } from '../../utils/version';
import { initiateEngineRestart } from '../../store/actions/chat-changes';
import { fetchWorkspaces } from '../../api/client';
import type { WorkspaceInfo } from '../../api/client';
import { formatShortTime } from '../../utils/formatTime';
import { isTauri } from '../../utils/platform';
import { invoke } from '../../utils/tauri';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { CloseIcon } from '../shared/icons';

export const controlPanelOpen = signal(false);

const apiUrl = typeof window !== 'undefined' && window.location ? window.location.origin : '';

export function controlPanelBadgeCount(): number {
  return (restartRequired.value ? 1 : 0) + (updateAvailable.value ? 1 : 0);
}

export function controlPanelBadgeTooltip(): string | undefined {
  const restart = restartRequired.value;
  const update = updateAvailable.value;
  if (restart && update) return 'Restart needed · Update available';
  if (restart) return 'Restart needed';
  if (update) return 'Update available';
  return undefined;
}

export function ControlPanel() {
  const ref = useRef<HTMLDivElement>(null);
  const [wsLoadable, setWsLoadable] = useState<Loadable<WorkspaceInfo[]>>({ status: 'not-loaded' });
  const open = controlPanelOpen.value;

  const status = connectionStatus.value;
  const connected = status === 'connected';
  const name = workspaceName.value;
  const path = workspacePath.value;
  const startedAt = engineStartedAt.value;
  const release = lucidosRelease.value;
  const engineVer = engineVersion.value;
  const latestEngineVer = latestEngineVersion.value;
  const latestTauriVer = latestTauriAppVersion.value;
  const tauriVersion = isTauri() ? window.__COGNOS_APP_VERSION__ : undefined;
  const restart = restartRequired.value;
  const update = updateAvailable.value;

  // Click outside + Escape to close — only register when open
  useEffect(() => {
    if (!open) return;
    function handleClick(e: MouseEvent) {
      const target = e.target as HTMLElement;
      if (target.closest('[data-role="control-panel-toggle"]')) return;
      if (ref.current && !ref.current.contains(target)) {
        controlPanelOpen.value = false;
      }
    }
    function handleKey(e: KeyboardEvent) {
      if (e.key === 'Escape') controlPanelOpen.value = false;
    }
    document.addEventListener('click', handleClick);
    document.addEventListener('keydown', handleKey);
    return () => {
      document.removeEventListener('click', handleClick);
      document.removeEventListener('keydown', handleKey);
    };
  }, [open]);

  // Fetch other workspaces on open
  useEffect(() => {
    if (!open || !connected) return;
    setWsLoadable({ status: 'loading' });
    fetchWorkspaces()
      .then(res => setWsLoadable({ status: 'loaded', data: res.workspaces }))
      .catch(e => setWsLoadable(toFailed(e)));
  }, [open, connected]);

  const handleRefresh = useCallback(() => {
    controlPanelOpen.value = false;
    window.location.reload();
  }, []);

  const handleRestart = useCallback(async () => {
    controlPanelOpen.value = false;
    const extraAction = isTauri()
      ? { label: 'Restart App', onClick: () => {
          invoke('restart_app').catch((e: unknown) => {
            showToast(`Failed to restart app: ${e}`, 'error');
          });
        } }
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
    navigator.clipboard.writeText(apiUrl).then(
      () => showToast('Copied to clipboard', 'success'),
      () => showToast('Failed to copy', 'error')
    );
  }, []);

  if (!open) return null;

  const hasEngineUpdate = engineVer && latestEngineVer && isNewerVersion(latestEngineVer, engineVer);
  const hasTauriUpdate = tauriVersion && latestTauriVer && isNewerVersion(latestTauriVer, tauriVersion);

  return (
    <div class="control-panel" ref={ref}>
      <div class="control-panel-section">
        <div class="control-panel-status-row">
          <span class={`status-dot ${status}`} />
          <span class="control-panel-status-text">
            {connected ? 'Connected' : 'Disconnected'}
          </span>
          <button
            class="icon-btn control-panel-close"
            aria-label="Close control panel"
            onClick={() => { controlPanelOpen.value = false; }}
          >
            <CloseIcon />
          </button>
        </div>
        {restart && (
          <div class="control-panel-notice">
            {hasEngineUpdate
              ? 'New engine version available — restart to activate'
              : 'Engine changes applied — restart to activate'}
          </div>
        )}
        {/* Restart subsumes refresh — no need to show both */}
        {update && !restart && (
          <div class="control-panel-notice">
            Client update available — refresh to activate
          </div>
        )}
        <div class="control-panel-info">
          <div class="control-panel-info-row">
            <span class="control-panel-label">Workspace</span>
            <span class="control-panel-value">{name || path || 'unknown'}</span>
          </div>
          {path && name && (
            <div class="control-panel-info-row">
              <span class="control-panel-label">Path</span>
              <span class="control-panel-value control-panel-path">{path}</span>
            </div>
          )}
          {release && (
            <div class="control-panel-info-row">
              <span class="control-panel-label">Lucidos</span>
              <span class="control-panel-value">{release}</span>
            </div>
          )}
          {engineVer && (
            <div class="control-panel-info-row">
              <span class="control-panel-label">Engine</span>
              <span class="control-panel-value">
                {engineVer}
                {hasEngineUpdate && (
                  <span class="control-panel-update"> (latest: {latestEngineVer})</span>
                )}
              </span>
            </div>
          )}
          {tauriVersion && (
            <div class="control-panel-info-row">
              <span class="control-panel-label">Tauri</span>
              <span class="control-panel-value">
                {tauriVersion}
                {hasTauriUpdate && (
                  <span class="control-panel-update"> (latest: {latestTauriVer})</span>
                )}
              </span>
            </div>
          )}
          {startedAt && (
            <div class="control-panel-info-row">
              <span class="control-panel-label">Uptime</span>
              <span class="control-panel-value">since {formatShortTime(new Date(startedAt))}</span>
            </div>
          )}
          <div class="control-panel-info-row">
            <span class="control-panel-label">API</span>
            <button class="control-panel-value control-panel-api-url accent-link" onClick={copyApiUrl}>
              {apiUrl}
            </button>
          </div>
        </div>
      </div>

      <div class="control-panel-section control-panel-actions">
        <button class="action-btn" onClick={handleRefresh}>
          Refresh Client
        </button>
        <button class="action-btn" onClick={handleRestart}>
          Rebuild &amp; Restart
        </button>
      </div>

      {connected && (
        <div class="control-panel-section control-panel-workspaces">
          <div class="control-panel-section-title">Other Workspaces</div>
          {wsLoadable.status === 'loading' && <div class="control-panel-empty">Loading...</div>}
          {wsLoadable.status === 'failed' && (
            <div class="control-panel-empty error-text">Failed to load workspaces</div>
          )}
          {wsLoadable.status === 'loaded' && wsLoadable.data.length === 0 && (
            <div class="control-panel-empty">No other workspaces running</div>
          )}
          {wsLoadable.status === 'loaded' && wsLoadable.data.map(ws => (
            <a
              key={ws.path}
              class="control-panel-workspace-row"
              href={ws.port ? `https://localhost:${ws.port}` : undefined}
              target="_blank"
              rel="noopener"
            >
              <span class={`status-dot ${ws.engine_running ? 'connected' : 'disconnected'}`} />
              <span class="control-panel-ws-name">{ws.name}</span>
              {ws.port && <span class="control-panel-ws-port">:{ws.port}</span>}
            </a>
          ))}
        </div>
      )}
    </div>
  );
}
