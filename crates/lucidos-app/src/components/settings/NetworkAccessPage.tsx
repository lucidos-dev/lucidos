import type { ComponentChildren } from 'preact';
import { useState, useEffect, useCallback } from 'preact/hooks';
import { showToast } from '../../store/store';
import { getNetworkConfig, setNetworkConfig } from '../../api/client/settings';
import type { NetworkConfigResponse } from '../../api/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { toFailed } from '../../store/types';
import type { Loadable } from '../../store/types';
import { errorDetail } from '../../utils/errorDetail';
import { LoadableError } from '../shared/LoadableError';
import {
  parseBindValue,
  toBindValue,
  isValidIp,
  isValidBindSelection,
  type BindMode,
} from '../../utils/bindMode';

const MODE_OPTIONS: Array<{ mode: BindMode; label: string; hint: string }> = [
  { mode: 'loopback', label: 'Loopback only', hint: 'This Mac only (most secure)' },
  { mode: 'address', label: 'Tailnet / specific address', hint: 'Bind one IP, e.g. your Tailscale address' },
  { mode: 'all', label: 'All interfaces', hint: 'LAN + tailnet' },
];

/** Human label for the effective inherited gateway bind shown when engines
 *  inherit (read-only here — changed from the workspace picker). */
function describeBind(value: string): string {
  const { mode, address } = parseBindValue(value);
  if (mode === 'loopback') return 'Loopback only';
  if (mode === 'all') return 'All interfaces';
  return address;
}

/**
 * Per-workspace engine Network access (Settings → Access → Network access).
 *
 * Controls THIS workspace's engine bind — the address its API server listens on.
 * The machine-global gateway bind + the "engines inherit gateway bind" toggle
 * live in the workspace picker (they apply before any workspace is selected). So
 * when engines inherit the gateway bind (the default), this pane is read-only and
 * shows the inherited value; the per-workspace control is live only when inherit
 * is turned off in the picker.
 *
 * Binds are set at process start — a change takes effect only after an engine
 * restart (we cannot re-bind a live socket), so the pane says so plainly.
 */
export function NetworkAccessPage() {
  const [info, setInfo] = useState<Loadable<NetworkConfigResponse>>({ status: 'not-loaded' });
  const [mode, setMode] = useState<BindMode>('loopback');
  const [address, setAddress] = useState('');
  const [saving, setSaving] = useState(false);
  const showLoading = useDelayedLoading(info);

  const reload = useCallback(() => {
    setInfo({ status: 'loading' });
    getNetworkConfig()
      .then((data) => {
        setInfo({ status: 'loaded', data });
        const parsed = parseBindValue(data.engine_bind);
        setMode(parsed.mode);
        setAddress(parsed.address);
      })
      .catch((e) => setInfo(toFailed(e)));
  }, []);

  useEffect(() => { reload(); }, [reload]);

  const onSave = useCallback(async () => {
    if (!isValidBindSelection(mode, address)) {
      showToast('Enter a valid IP address', 'error');
      return;
    }
    setSaving(true);
    try {
      const result = await setNetworkConfig({ engine_bind: toBindValue(mode, address) });
      if (!result.success) {
        showToast(result.error || 'Failed to save network bind', 'error');
        return;
      }
      showToast('Saved — restart the engine to apply', 'info');
      reload();
    } catch (e) {
      showToast(`Failed to save network bind: ${errorDetail(e)}`, 'error');
    } finally {
      setSaving(false);
    }
  }, [mode, address, reload]);

  // The section shell and its TITLE render in every state, because the title
  // carries `data-search-anchor="access:network"` and that anchor is a
  // navigation target: Search Everywhere jumps here, and `SettingsView`'s scroll
  // effect does ONE `querySelector` on the commit where the subview mounts, then
  // clears the target whether it found the element or not. An anchor that only
  // appears once an async fetch lands is therefore missed on a cold open, and
  // the user is dropped at the top of the page with no scroll and no marker.
  // Every other settings anchor renders unconditionally; this one must too.
  const shell = (body: ComponentChildren) => (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="access:network">
        Network access
      </div>
      {body}
    </div>
  );

  if (info.status === 'failed') {
    return shell(<LoadableError noun="network configuration" error={info.error} />);
  }
  if (info.status !== 'loaded') {
    // Still gated on the delay so a fast load never flashes a loader, but the
    // shell itself is unconditional (see above).
    return shell(showLoading ? <div class="empty-state">Loading…</div> : null);
  }

  const data = info.data;
  const inherited = data.inherit;
  const addressInvalid = mode === 'address' && address.trim() !== '' && !isValidIp(address);
  const stored = parseBindValue(data.engine_bind);
  const dirty =
    !inherited && (mode !== stored.mode || (mode === 'address' && address.trim() !== stored.address));
  const placeholder = data.detected_tailscale_ip ?? '100.x.y.z';

  return shell(
    <>
      <p class="settings-section-desc">
        Controls the address this workspace's engine listens on. Loopback keeps it
        reachable only from this Mac; binding your Tailscale address exposes it to
        your tailnet so you can use it from your phone.
      </p>

      {inherited ? (
        <div class="list-rows">
          <div class="list-row">
            <div class="list-row-info">
              <div class="title">Inheriting the machine-wide bind</div>
              <div class="list-row-details list-row-details-prose">
                Engines follow the gateway's bind:{' '}
                <strong>{describeBind(data.gateway_bind)}</strong>. To set a bind
                just for this workspace, turn off “Engines inherit gateway bind” in
                the workspace picker's Network access control.
              </div>
            </div>
          </div>
        </div>
      ) : (
        <>
          <div class="settings-row-options" role="radiogroup" aria-label="Engine network bind">
            {MODE_OPTIONS.map((opt) => (
              <button
                key={opt.mode}
                type="button"
                role="radio"
                aria-checked={mode === opt.mode}
                class={`settings-option${mode === opt.mode ? ' active' : ''}`}
                data-tooltip={opt.hint}
                onClick={() => setMode(opt.mode)}
              >
                {opt.label}
              </button>
            ))}
          </div>

          {mode === 'address' && (
            <div class="settings-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: '0.25rem' }}>
              <input
                class="device-name-input"
                type="text"
                placeholder={placeholder}
                value={address}
                aria-invalid={addressInvalid}
                aria-label="Bind IP address"
                onInput={(e) => setAddress((e.target as HTMLInputElement).value)}
              />
              {addressInvalid ? (
                <span class="settings-field-error">Not a valid IP address.</span>
              ) : (
                <span class="settings-section-desc" style={{ margin: 0 }}>
                  {data.detected_tailscale_ip
                    ? `Detected Tailscale address: ${data.detected_tailscale_ip}`
                    : 'Enter the IP to bind (your Tailscale 100.x address, or a LAN IP).'}
                </span>
              )}
            </div>
          )}

          <div class="system-notice" style={{ marginTop: '0.75rem' }}>
            Changes take effect after the engine restarts — Lucidos can't re-bind a
            live socket.
          </div>

          <div class="system-actions">
            <button
              class="action-btn action-btn-confirm"
              disabled={saving || !dirty || addressInvalid || (mode === 'address' && address.trim() === '')}
              onClick={onSave}
            >
              {saving ? 'Saving…' : 'Save'}
            </button>
          </div>
        </>
      )}
    </>
  );
}
