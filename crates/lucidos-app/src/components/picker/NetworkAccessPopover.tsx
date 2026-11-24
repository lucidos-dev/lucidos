
import type { Loadable } from '../../store/types';
import type { GatewayNetworkConfig } from '../../api/client/control';
import {
  isValidIp,
  isValidBindSelection,
  bindDraftMatchesSaved,
  type BindDraft,
  type BindMode,
} from '../../utils/bindMode';

export interface NetworkEditor {
  config: GatewayNetworkConfig;
  draft: BindDraft;
}

const BIND_MODES: [BindMode, string][] = [
  ['loopback', 'Loopback only'],
  ['address', 'Tailnet / IP'],
  ['all', 'All interfaces'],
];

export interface NetworkAccessBodyProps {
  state: Loadable<NetworkEditor>;
  saving: boolean;
  busy: boolean;
  onMode: (mode: BindMode) => void;
  onAddress: (address: string) => void;
  onInherit: (inherit: boolean) => void;
  onFillDetected: () => void;
  onRetry: () => void;
  onCancel: () => void;
  onSave: () => void;
}

export function networkAccessBody({
  state,
  saving,
  busy,
  onMode,
  onAddress,
  onInherit,
  onFillDetected,
  onRetry,
  onCancel,
  onSave,
}: NetworkAccessBodyProps) {
  const editor = state.status === 'loaded' ? state.data : null;
  const draft = editor?.draft ?? null;
  const detected = editor?.config.detected_tailscale_ip ?? null;
  const phase =
    state.status === 'loaded' ? 'ready' : state.status === 'failed' ? 'failed' : 'loading';
  const addressOpen = draft?.mode === 'address';
  const addressInvalid = draft !== null && draft.address.trim() !== '' && !isValidIp(draft.address);

  const saveBlockedReason =
    editor === null || draft === null
      ? null
      : !isValidBindSelection(draft.mode, draft.address)
        ? 'Enter an IP address'
        : bindDraftMatchesSaved(draft, editor.config.gateway_bind, editor.config.inherit)
          ? 'No changes to save'
          : null;
  const canSave = draft !== null && saveBlockedReason === null;

  return (
    <>
      <h2 class="ws-picker-net-title">Network access</h2>
      <p class="ws-picker-net-desc">
        How this machine's Lucidos is reachable. The gateway fronts every
        workspace, and the setting is machine-wide: every Lucidos install here
        shares it.
      </p>
      <div class="ws-picker-net-controls" data-state={phase} aria-busy={phase === 'loading'}>
        <div class="ws-picker-net-modes" role="radiogroup" aria-label="Gateway bind">
          {BIND_MODES.map(([m, label]) => {
            const active = draft?.mode === m;
            return (
              <button
                key={m}
                type="button"
                role="radio"
                aria-checked={active}
                class={`ws-picker-net-mode${active ? ' active' : ''}`}
                disabled={busy || saving || draft === null}
                onClick={() => onMode(m)}
              >
                {label}
              </button>
            );
          })}
        </div>
        <div
          class={`ws-picker-net-collapse${addressOpen ? ' is-open' : ''}`}
          aria-hidden={!addressOpen}
        >
          <div class="ws-picker-net-address">
            <input
              class="ws-picker-input"
              type="text"
              placeholder={detected ?? '100.x.y.z'}
              value={draft?.address ?? ''}
              aria-invalid={addressInvalid}
              aria-label="Gateway bind IP address"
              disabled={!addressOpen || busy || saving}
              onInput={(e) => onAddress((e.target as HTMLInputElement).value)}
            />
            {addressInvalid ? (
              <span class="ws-picker-net-error">Not a valid IP address.</span>
            ) : detected ? (
              <span class="ws-picker-net-hint">
                Detected Tailscale:{' '}
                <button
                  type="button"
                  class="ws-picker-net-detected"
                  data-tooltip="Use this address"
                  tabIndex={addressOpen ? undefined : -1}
                  onClick={onFillDetected}
                >
                  {detected}
                </button>
              </span>
            ) : (
              <span class="ws-picker-net-hint">Your Tailscale 100.x address, or a LAN IP.</span>
            )}
          </div>
        </div>
        <label class="ws-picker-net-toggle">
          <input
            type="checkbox"
            checked={draft?.inherit ?? false}
            disabled={busy || saving || draft === null}
            onChange={(e) => onInherit((e.target as HTMLInputElement).checked)}
          />
          <span>Engines inherit gateway bind</span>
        </label>
      </div>
      <div
        class={`ws-picker-net-collapse${phase === 'failed' ? ' is-open' : ''}`}
        aria-hidden={phase !== 'failed'}
      >
        <div class="ws-picker-net-failure">
          <p class="ws-picker-net-error">
            Could not read this machine's network config
            {state.status === 'failed' ? `: ${state.error}` : ''}.{' '}
            <button
              type="button"
              class="ws-picker-net-retry"
              tabIndex={phase === 'failed' ? undefined : -1}
              onClick={onRetry}
            >
              Retry
            </button>
          </p>
        </div>
      </div>
      <p class="ws-picker-net-hint">
        When off, each workspace sets its own engine bind in its Settings →
        Network access.
      </p>
      <p class="ws-picker-net-restart">Takes effect after the gateway / engine restarts.</p>
      <div class="ws-picker-confirm-actions">
        {saveBlockedReason && !saving && (
          <span class="ws-picker-net-blocked">{saveBlockedReason}</span>
        )}
        <button class="ws-picker-btn" onClick={onCancel}>
          Cancel
        </button>
        <button
          class="ws-picker-btn ws-picker-btn-confirm"
          disabled={busy || saving || !canSave}
          onClick={onSave}
        >
          {saving ? 'Saving…' : 'Save'}
        </button>
      </div>
    </>
  );
}
