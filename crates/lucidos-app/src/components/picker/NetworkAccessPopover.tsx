/**
 * Body of the picker's machine-global **Network access** popover: the gateway
 * bind (loopback / tailnet-or-IP / all interfaces) plus the "engines inherit
 * gateway bind" toggle. Both are persisted to `~/.lucidos/network.toml` by the
 * gateway control plane, so the value is shared by every Lucidos gateway on
 * this machine.
 *
 * Split out of `WorkspacePicker` as a PURE function of its state so the
 * load/settle behaviour is unit-testable (same shape as `directoryPickerBody`).
 *
 * Two invariants this file exists to hold:
 *
 * 1. **It shows the SAVED bind, never a leftover edit.** The draft lives INSIDE
 *    the loaded state (`NetworkEditor`), so it cannot outlive the config it was
 *    seeded from: reopening resets to `loading`, which carries no draft at all,
 *    and no mode is marked active until the refetch lands. The previous shape
 *    kept the config and the three draft fields in four independent signals
 *    that were never reset, so cancelling an edit and reopening rendered the
 *    abandoned click as the active mode until the refetch corrected it.
 * 2. **Nothing jumps once it is on screen.** The structure renders at full
 *    height immediately; afterwards only opacity, active state, and one
 *    height-animated collapse change. So settling on a saved IP grows the
 *    popover smoothly instead of snapping (see `.ws-picker-net-collapse`).
 */

import type { Loadable } from '../../store/types';
import type { GatewayNetworkConfig } from '../../api/client/control';
import {
  isValidIp,
  isValidBindSelection,
  bindDraftMatchesSaved,
  type BindDraft,
  type BindMode,
} from '../../utils/bindMode';

/** The saved config together with the edit in progress against it. Pairing them
 *  in one value is what makes a stale draft unrepresentable. */
export interface NetworkEditor {
  config: GatewayNetworkConfig;
  draft: BindDraft;
}

/** Segment order + labels of the three-mode bind picker. */
const BIND_MODES: [BindMode, string][] = [
  ['loopback', 'Loopback only'],
  ['address', 'Tailnet / IP'],
  ['all', 'All interfaces'],
];

export interface NetworkAccessBodyProps {
  /** Saved config + draft, or why there isn't one yet. */
  state: Loadable<NetworkEditor>;
  /** A save is in flight. */
  saving: boolean;
  /** The picker is globally busy (another action is running). */
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
  // Anything other than `loaded` is unsettled: the value-bearing controls dim
  // and go inert, and NO mode is marked active. That is the whole guarantee
  // that the popover never shows a bind other than the saved one.
  const phase =
    state.status === 'loaded' ? 'ready' : state.status === 'failed' ? 'failed' : 'loading';
  const addressOpen = draft?.mode === 'address';
  const addressInvalid = draft !== null && draft.address.trim() !== '' && !isValidIp(draft.address);

  // Why Save is unavailable, stated next to the button. A greyed control with
  // no reason is what led the user to read the button's own state as evidence
  // of what was stored ("it offers Save, so this is not my config").
  //
  // `null` means Save is available. While unsettled there is no reason line at
  // all: the whole control block is visibly dimmed and inert, which already
  // says "not ready" without a second, competing explanation.
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
      {/* The form STRUCTURE renders immediately at a constant height; only the
          config-dependent VALUES wait for the load, and the one part whose
          height depends on the value (the address field) animates. While
          unsettled the controls dim + go inert with nothing marked active, so
          no wrong default is ever shown. */}
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
        {/* Always mounted so the popover can animate to its settled height
            rather than snapping when the saved bind turns out to be an IP. */}
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
              // `disabled` already takes it out of the tab order while the row
              // is collapsed; the sibling buttons below are not disabled, so
              // they need the explicit tabIndex.
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
      {/* A failed load is stated in place with a retry, rather than leaving the
          controls silently dimmed forever. Collapsed (not unmounted) so it
          animates in like the address row. */}
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
