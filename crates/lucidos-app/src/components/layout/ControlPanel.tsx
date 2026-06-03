import { useState, useEffect, useRef, useCallback } from 'preact/hooks';
import { signal } from '@preact/signals';
import { connectionStatus, workspaceName, workspacePath, engineStartedAt, lucidosRelease, lucidosReleaseDirty, engineVersion, latestEngineVersion, latestTauriAppVersion, restartRequired, updateAvailable, restartGroups, showConfirm, showToast } from '../../store/store';
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
import { useDismissOnOutside } from '../../hooks/useAnchoredPopover';
import { viewportIsMobile } from '../../utils/viewport';
import { ENGINE_VERSION } from 'virtual:engine-version';

export const controlPanelOpen = signal(false);
/** Anchor element of the toggle that opened the panel, so dismiss-on-outside
 *  treats re-clicking the same toggle as "inside" (toggle's own onClick
 *  handles the close). Set by the brand-label `data-role="control-panel-toggle"`
 *  in AppHeader / MobileAppHeader at open time. */
export const controlPanelAnchor = signal<HTMLElement | null>(null);

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

export function ControlPanel({ layout }: { layout: 'desktop' | 'mobile' }) {
  const ref = useRef<HTMLDivElement>(null);
  const [wsLoadable, setWsLoadable] = useState<Loadable<WorkspaceInfo[]>>({ status: 'not-loaded' });
  const open = controlPanelOpen.value;
  // Both AppHeader and MobileAppHeader render simultaneously (dual-layout) and
  // each mounts a ControlPanel. Without this gate, the hidden copy's dismiss
  // hook treats clicks on the visible panel's buttons as "outside" and
  // swallows them — Restart/Refresh/X would never fire.
  const isActiveLayout = layout === (viewportIsMobile.value ? 'mobile' : 'desktop');
  const effectiveOpen = open && isActiveLayout;

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
  const tauriClientVersion = isTauri() ? window.__LUCIDOS_APP_VERSION__ : undefined;
  const clientVersion = tauriClientVersion ?? ENGINE_VERSION;
  const restart = restartRequired.value;
  const update = updateAvailable.value;

  // Anchor is the brand-label that opened the panel — re-clicking it routes
  // through its own onClick toggle. Escape comes free with the hook.
  useDismissOnOutside(effectiveOpen, ref, controlPanelAnchor.value, () => {
    controlPanelOpen.value = false;
  });

  // Fetch other workspaces on open. Gated to the active layout so the dual-
  // mount doesn't double-fetch on every open.
  useEffect(() => {
    if (!effectiveOpen || !connected) return;
    setWsLoadable({ status: 'loading' });
    fetchWorkspaces()
      .then(res => setWsLoadable({ status: 'loaded', data: res.workspaces }))
      .catch(e => setWsLoadable(toFailed(e)));
  }, [effectiveOpen, connected]);

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

  if (!effectiveOpen) return null;

  const hasEngineUpdate = engineVer && latestEngineVer && isNewerVersion(latestEngineVer, engineVer);
  // Tauri shells compare against the on-disk Tauri app version. Web compares
  // against the engine version on disk — the bundled ENGINE_VERSION is what
  // engine VERSION was when this bundle loaded, so a newer disk value means
  // the user should refresh to pick up the rebuilt bundle.
  const latestClientVer = tauriClientVersion ? latestTauriVer : latestEngineVer;
  const hasClientUpdate = clientVersion && latestClientVer && isNewerVersion(latestClientVer, clientVersion);

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
              <span class="control-panel-value">
                {release}
                {releaseDirty && <span class="control-panel-update">*</span>}
              </span>
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
          {clientVersion && (
            <div class="control-panel-info-row">
              <span class="control-panel-label">Client</span>
              <span class="control-panel-value">
                {clientVersion}
                {hasClientUpdate && (
                  <span class="control-panel-update"> (latest: {latestClientVer})</span>
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
          {release && releaseDirty && (
            <div class="control-panel-footnote">
              <span class="control-panel-update">*</span> code has changed since the {release} release
            </div>
          )}
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
