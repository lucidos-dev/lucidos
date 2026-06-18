import { useState, useEffect, useCallback } from 'preact/hooks';
import { showToast } from '../../store/store';
import { isTauri } from '../../utils/platform';
import {
  getConnectInfo,
  tailscaleUp,
  tailscaleServe,
  openExternal,
  type ConnectInfo,
} from '../../utils/tauri';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { toFailed } from '../../store/types';
import type { Loadable } from '../../store/types';
import { errorDetail } from '../../utils/errorDetail';
import { LoadableError } from '../shared/LoadableError';

/** A connect URL with a copy-to-clipboard button. */
function UrlRow({ label, url, hint }: { label: string; url: string; hint?: string }) {
  const copy = useCallback(() => {
    navigator.clipboard.writeText(url).then(
      () => showToast('Copied to clipboard', 'success'),
      () => showToast('Failed to copy', 'error'),
    );
  }, [url]);
  return (
    <div class="list-row">
      <div class="list-row-info">
        <div class="title">{label}</div>
        <div class="list-row-details">
          <button class="mobile-access-url-button accent-link" onClick={copy}>{url}</button>
          {hint && <> &middot; {hint}</>}
        </div>
      </div>
      <div class="list-row-actions">
        <button class="action-btn" onClick={copy}>Copy</button>
      </div>
    </div>
  );
}

/**
 * Mobile Access settings (packaged desktop app only).
 *
 * Surfaces the engine's connect URLs (localhost / LAN / Tailscale) so the user
 * knows what to open on the phone, and drives the Tailscale setup they consent
 * to here. We use `tailscale serve` (tailnet-private HTTPS), never `funnel`
 * (public) — the engine has no inbound API auth.
 */
export function MobileAccessPage() {
  const [info, setInfo] = useState<Loadable<ConnectInfo>>({ status: 'not-loaded' });
  const [busy, setBusy] = useState<null | 'up' | 'serve'>(null);
  const [authKey, setAuthKey] = useState('');
  const showLoading = useDelayedLoading(info);

  const reload = useCallback(() => {
    if (!isTauri()) {
      setInfo(toFailed(new Error('Mobile access is only available in the desktop app.')));
      return;
    }
    setInfo({ status: 'loading' });
    getConnectInfo()
      .then(data => setInfo({ status: 'loaded', data }))
      .catch(e => setInfo(toFailed(e)));
  }, []);

  useEffect(() => { reload(); }, [reload]);

  const onUp = useCallback(async () => {
    setBusy('up');
    try {
      await tailscaleUp(authKey.trim() || undefined);
      setAuthKey('');
      reload();
    } catch (e) {
      showToast(`Tailscale sign-in failed: ${errorDetail(e)}`, 'error');
    } finally {
      setBusy(null);
    }
  }, [authKey, reload]);

  const onServe = useCallback(async () => {
    setBusy('serve');
    try {
      const url = await tailscaleServe();
      showToast(`Engine exposed at ${url}`, 'success');
      reload();
    } catch (e) {
      showToast(`Tailscale serve failed: ${errorDetail(e)}`, 'error');
    } finally {
      setBusy(null);
    }
  }, [reload]);

  const openTailscaleDownload = useCallback(() => {
    // open in the system browser, not the embedded webview
    openExternal('https://tailscale.com/download').catch(() => {
      window.open('https://tailscale.com/download', '_blank', 'noopener');
    });
  }, []);

  if (info.status === 'failed') {
    return (
      <div class="settings-section">
        <div class="settings-section-title">Mobile Access</div>
        <LoadableError noun="connect info" error={info.error} />
      </div>
    );
  }
  if (info.status !== 'loaded') {
    if (!showLoading) return null;
    return (
      <div class="settings-section">
        <div class="settings-section-title">Mobile Access</div>
        <div class="empty-state">Loading…</div>
      </div>
    );
  }

  const ts = info.data.tailscale;

  return (
    <>
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="mobile-access:urls">Connect URLs</div>
        <p class="settings-section-desc">
          The engine runs as an always-on background service and is reachable at these addresses.
          Open one on another device to use Lucidos from your phone.
        </p>
        <div class="list-rows">
          <UrlRow label="This Mac" url={info.data.localhost_url} hint="localhost" />
          {info.data.lan_url
            ? <UrlRow label="Local network" url={info.data.lan_url} hint="same Wi-Fi only" />
            : (
              <div class="list-row">
                <div class="list-row-info">
                  <div class="title">Local network</div>
                  <div class="list-row-details">No LAN address detected</div>
                </div>
              </div>
            )}
          {ts.url && <UrlRow label="Tailscale" url={ts.url} hint="anywhere, HTTPS + push" />}
        </div>
      </div>

      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="mobile-access:tailscale">Tailscale (recommended)</div>
        <p class="settings-section-desc">
          Tailscale gives your phone a private, encrypted HTTPS link to this Mac that works off
          your home network — enabling the installable PWA and push notifications. It stays
          tailnet-private (we use <code>serve</code>, not <code>funnel</code>), so the engine is
          never exposed to the public internet.
        </p>

        {!ts.installed && (
          <div class="list-rows">
            <div class="list-row">
              <div class="list-row-info">
                <div class="title">Install Tailscale</div>
                <div class="list-row-details">
                  Tailscale is a system VPN, so macOS asks for your approval during install.
                </div>
              </div>
              <div class="list-row-actions">
                <button class="action-btn" onClick={openTailscaleDownload}>Get Tailscale</button>
              </div>
            </div>
          </div>
        )}

        {ts.installed && !ts.running && (
          <div class="list-rows">
            <div class="list-row repo-add-form">
              <div class="list-row-info" style={{ gap: '0.5rem' }}>
                <div class="title">Sign in to Tailscale</div>
                <div class="list-row-details">
                  Opens a browser to join your tailnet. Optionally paste a pre-authorized auth key.
                </div>
                <input
                  class="device-name-input"
                  type="text"
                  placeholder="Auth key (optional) — tskey-auth-…"
                  value={authKey}
                  onInput={(e) => setAuthKey((e.target as HTMLInputElement).value)}
                />
              </div>
              <div class="list-row-actions">
                <button class="action-btn action-btn-confirm" disabled={busy === 'up'} onClick={onUp}>
                  {busy === 'up' ? 'Signing in…' : 'Sign in'}
                </button>
              </div>
            </div>
          </div>
        )}

        {ts.installed && ts.running && (
          <div class="list-rows">
            <div class="list-row">
              <div class="list-row-info">
                <div class="title">Expose the engine over Tailscale</div>
                <div class="list-row-details">
                  {ts.hostname
                    ? <>Serves the engine at <strong>{ts.url}</strong> with an auto-renewed cert.</>
                    : 'Sets up tailnet HTTPS for the engine.'}
                </div>
              </div>
              <div class="list-row-actions">
                <button class="action-btn action-btn-confirm" disabled={busy === 'serve'} onClick={onServe}>
                  {busy === 'serve' ? 'Setting up…' : (ts.url ? 'Re-apply' : 'Expose')}
                </button>
              </div>
            </div>
          </div>
        )}
      </div>

      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="mobile-access:phone">On your phone</div>
        <ol class="settings-section-desc" style={{ paddingLeft: '1.25rem', lineHeight: 1.7 }}>
          <li>Install the Tailscale app and sign in to the <strong>same tailnet</strong> as this Mac.</li>
          <li>Open the <strong>Tailscale</strong> URL above (copy it / send it to yourself).</li>
          <li>Add it to your home screen to install the Lucidos PWA and enable push.</li>
        </ol>
      </div>
    </>
  );
}
