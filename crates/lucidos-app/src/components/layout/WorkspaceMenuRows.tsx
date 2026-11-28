import { useSignal } from '@preact/signals';
import { restartRequired, engineVersionReady, updateAvailable, engineNewVersionReady } from '../../store/store';
import { confirmAndRestartEngine, initiateEngineRestart } from '../../store/actions/chat-changes';
import { refreshClient } from '../../hooks/sw-update';
import { ReloadIcon, PowerIcon } from '../shared/icons';

export function refreshRowState(
  clientUpdateAvailable: boolean,
  enginePending: boolean,
): { tooltip: string; showUpdateBadge: boolean } {
  const update = clientUpdateAvailable && !enginePending;
  const base = 'Reload the client';
  return { tooltip: update ? `Update available · ${base}` : base, showUpdateBadge: update };
}

export function restartRowState(
  ready: boolean,
): { tooltip: string; pending: boolean; badge: string | null } {
  return {
    tooltip: ready ? 'Restart onto the new version' : 'Restart this workspace',
    pending: ready,
    badge: ready ? 'New version' : null,
  };
}

export function WorkspaceRefreshRow({ onClose }: { onClose: () => void }) {
  const enginePending = restartRequired.value || engineVersionReady.value;
  const { tooltip, showUpdateBadge } = refreshRowState(updateAvailable.value, enginePending);

  return (
    <button
      type="button"
      class="brand-menu-item brand-menu-refresh"
      role="menuitem"
      aria-label={tooltip}
      data-tooltip={tooltip}
      onClick={() => { onClose(); refreshClient(); }}
    >
      <ReloadIcon />
      Refresh
      {showUpdateBadge && <span class="brand-menu-refresh-badge" aria-hidden="true" />}
    </button>
  );
}

export function WorkspaceRestartRow({ onClose }: { onClose: () => void }) {
  const confirming = useSignal(false);
  const { tooltip, pending, badge } = restartRowState(engineNewVersionReady());

  if (confirming.value) {
    return (
      <div class={`brand-menu-item brand-menu-confirm-row${pending ? ' is-pending' : ''}`} role="none">
        <PowerIcon />
        Restart
        <span class="brand-menu-confirm-actions">
          <button
            type="button"
            class="brand-menu-confirm"
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              confirming.value = false;
              onClose();
              if (engineNewVersionReady()) {
                void confirmAndRestartEngine();
                return;
              }
              void initiateEngineRestart();
            }}
          >
            OK
          </button>
        </span>
      </div>
    );
  }

  return (
    <button
      type="button"
      class={`brand-menu-item${pending ? ' is-pending' : ''}`}
      role="menuitem"
      aria-label={tooltip}
      data-tooltip={tooltip}
      onClick={() => {
        if (pending) {
          onClose();
          void confirmAndRestartEngine();
          return;
        }
        confirming.value = true;
      }}
    >
      <PowerIcon />
      Restart
      {badge && (
        <span class="brand-menu-value brand-menu-restart-badge">
          <span class="brand-menu-value-name">{badge}</span>
        </span>
      )}
    </button>
  );
}
