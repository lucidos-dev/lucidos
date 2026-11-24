import type { ComponentChildren } from 'preact';
import { useState, useEffect, useCallback } from 'preact/hooks';
import { showToast } from '../../store/store';
import { getNetworkConfig, setNetworkConfig } from '../../api/client/settings';
import type { NetworkConfigResponse } from '../../api/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { loadingIfFresh, toFailed } from '../../store/types';
import type { Loadable } from '../../store/types';
import { errorDetail } from '../../utils/errorDetail';
import { LoadableError } from '../shared/LoadableError';
import { Explainer } from '../shared/Explainer';
import { LoadingFade } from '../shared/LoadingFade';
import { SkText, SkeletonProvider } from '../shared/Skeleton';
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

function describeBind(value: string): string {
  const { mode, address } = parseBindValue(value);
  if (mode === 'loopback') return 'Loopback only';
  if (mode === 'all') return 'All interfaces';
  return address;
}

function InheritedBindRow({ gatewayBind }: { gatewayBind?: string }) {
  return (
    <div class="list-rows">
      <div class="list-row">
        <div class="list-row-info">
          <SkText class="title" as="div" w="14rem">Inheriting the machine-wide bind</SkText>
          <SkText class="list-row-details list-row-details-prose" as="div" w="90%">
            {gatewayBind !== undefined && (
              <>
                Engines follow the gateway's bind:{' '}
                <strong>{describeBind(gatewayBind)}</strong>. To set a bind
                just for this workspace, turn off “Engines inherit gateway bind” in
                the workspace picker's Network access control.
              </>
            )}
          </SkText>
        </div>
      </div>
    </div>
  );
}

export function NetworkAccessPage() {
  const [info, setInfo] = useState<Loadable<NetworkConfigResponse>>({ status: 'not-loaded' });
  const [mode, setMode] = useState<BindMode>('loopback');
  const [address, setAddress] = useState('');
  const [saving, setSaving] = useState(false);
  const showLoading = useDelayedLoading(info);

  const reload = useCallback((): Promise<void> => {
    setInfo(loadingIfFresh);
    return getNetworkConfig()
      .then((data) => {
        setInfo({ status: 'loaded', data });
        const parsed = parseBindValue(data.engine_bind);
        setMode(parsed.mode);
        setAddress(parsed.address);
      })
      .catch((e) => setInfo(toFailed(e)));
  }, []);

  useEffect(() => { void reload(); }, [reload]);
  const served = info.status === 'loaded' ? parseBindValue(info.data.engine_bind) : null;
  const editing = served !== null && (served.mode !== mode || served.address !== address);
  usePanelRefresh('network access', editing ? null : reload);

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
      void reload();
    } catch (e) {
      showToast(`Failed to save network bind: ${errorDetail(e)}`, 'error');
    } finally {
      setSaving(false);
    }
  }, [mode, address, reload]);

  const shell = (body: ComponentChildren) => (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="access:network">
        Network access
        <Explainer title="Network access">
          <p>Controls the address this workspace's engine listens on.</p>
          <p>
            Loopback keeps it reachable only from this Mac; binding your Tailscale
            address exposes it to your tailnet so you can use it from your phone.
          </p>
        </Explainer>
      </div>
      {body}
    </div>
  );

  if (info.status === 'failed') {
    return shell(<LoadableError noun="network configuration" error={info.error} />);
  }
  return shell(
    <LoadingFade
      showSkeleton={showLoading}
      skeleton={<SkeletonProvider><InheritedBindRow /></SkeletonProvider>}
    >
      {info.status === 'loaded' ? loadedBody(info.data) : null}
    </LoadingFade>,
  );

  function loadedBody(data: NetworkConfigResponse) {
    const inherited = data.inherit;
    const addressInvalid = mode === 'address' && address.trim() !== '' && !isValidIp(address);
    const stored = parseBindValue(data.engine_bind);
    const dirty =
      !inherited && (mode !== stored.mode || (mode === 'address' && address.trim() !== stored.address));
    const placeholder = data.detected_tailscale_ip ?? '100.x.y.z';

    return (
      <>
        {inherited ? (
          <InheritedBindRow gatewayBind={data.gateway_bind} />
        ) : (
          <>
            <div class="segmented-control" role="group" aria-label="Engine network bind">
              {MODE_OPTIONS.map((opt) => (
                <button
                  key={opt.mode}
                  type="button"
                  aria-pressed={mode === opt.mode}
                  class={`segmented-btn${mode === opt.mode ? ' active' : ''}`}
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
              Changes take effect after the engine restarts: Lucidos can't re-bind a
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
}
