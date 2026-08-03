import { useState, useEffect, useCallback } from 'preact/hooks';
import { showToast, enginePackaged, tailscaleServeRun } from '../../store/store';
import {
  beginTailscaleServeRun,
  clearTailscaleServeRun,
  applyTailscaleServeProgress,
} from '../../store/actions/backgroundActivity';
import { isTauri, isIOS, isAndroid, isStandalone } from '../../utils/platform';
import { openExternalUrl } from '../../utils/openExternalUrl';
import {
  getConnectInfo,
  tailscaleUp,
  tailscaleServe,
  openExternal,
  type ConnectInfo,
  type TailscaleInfo,
} from '../../utils/tauri';
import { getNetworkConfig } from '../../api/client';
import { openSettingsSubview } from '../../store/actions/menu';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { toFailed } from '../../store/types';
import type { Loadable } from '../../store/types';
import { errorDetail } from '../../utils/errorDetail';
import { LoadableError } from '../shared/LoadableError';

const TAILSCALE_DOWNLOAD_URL = 'https://tailscale.com/download';
const TAILSCALE_IOS_URL = 'https://apps.apple.com/app/tailscale/id1470499037';
const TAILSCALE_ANDROID_URL = 'https://play.google.com/store/apps/details?id=com.tailscale.ipn';

/** Pure: where "Get Tailscale" should send THIS device.
 *
 *  Half the readers of this page are holding the phone they need to install on,
 *  so the desktop download page is the wrong destination for them. Takes the
 *  platform flags as arguments so the routing is testable without a UA. */
export function tailscaleDownloadUrl(platform: { ios: boolean; android: boolean }): string {
  if (platform.ios) return TAILSCALE_IOS_URL;
  if (platform.android) return TAILSCALE_ANDROID_URL;
  return TAILSCALE_DOWNLOAD_URL;
}

/** Send the "Get Tailscale" button somewhere the user can actually install from.
 *
 *  Branches on `isTauri()` rather than catching the OS opener's rejection: the
 *  IPC bridge dereferences `window.__TAURI_INTERNALS__!` SYNCHRONOUSLY
 *  (`utils/tauri.ts`) and `openExternal` is not `async`, so off Tauri it THROWS
 *  instead of rejecting and no `.catch()` fallback ever runs. This page is
 *  reached from a phone, so that dead fallback meant the browser/PWA tap did
 *  nothing at all. Module-level (not a `useCallback`) so the routing is
 *  testable without standing the page up. */
export function openTailscaleDownload(): void {
  const url = tailscaleDownloadUrl({ ios: isIOS(), android: isAndroid() });
  if (!isTauri()) {
    // A new tab, or the real Safari app on an installed iOS PWA.
    openExternalUrl(url);
    return;
  }
  // Desktop: the system browser, not the embedded webview.
  openExternal(url).catch((e) => {
    showToast(`Couldn't open ${url}: ${errorDetail(e)}`, 'error');
  });
}

/** What the "Local network" row should show, derived from the gateway's
 *  configured bind. Loopback (the packaged default) means a LAN URL would be
 *  DEAD — the gateway only listens on this Mac — so the row must point at the
 *  Network access setting instead of advertising an unreachable URL. */
export type LanRowState =
  | { kind: 'disabled' }
  | { kind: 'url'; url: string }
  | { kind: 'none' };

/** Pure: derive the LAN row from the configured gateway bind. A specific-IP
 *  bind serves exactly that IP (plus loopback), so the row shows the bound
 *  IP's URL rather than the detected LAN IP (which may be a different,
 *  unreachable interface). Config-based, same "takes effect after restart"
 *  caveat as Settings → Network access. */
export function lanRowState(gatewayBind: string, lanIp: string | null, port: number): LanRowState {
  const bind = gatewayBind.trim();
  if (bind === '' || bind === 'loopback') return { kind: 'disabled' };
  if (bind === 'all') {
    return lanIp ? { kind: 'url', url: `http://${lanIp}:${port}` } : { kind: 'none' };
  }
  return { kind: 'url', url: `http://${bind}:${port}` };
}

/** Pure: the plain-HTTP tailnet URL, but only when the gateway actually serves
 *  that address.
 *
 *  Same rule as {@link lanRowState}, applied to the tailnet address: being on a
 *  tailnet says nothing about whether the gateway is listening on it. The
 *  packaged default binds loopback only, where `http://100.x:<port>` is as dead
 *  as the LAN URL that row already refuses to print. A specific-IP bind serves
 *  exactly that address, so it counts only when it IS the tailnet address. */
export function tailnetHttpUrl(
  gatewayBind: string,
  tailnetIp: string | null,
  port: number,
): string | null {
  if (!tailnetIp) return null;
  const bind = gatewayBind.trim();
  return bind === 'all' || bind === tailnetIp ? `http://${tailnetIp}:${port}` : null;
}

/** Pure: the two plain-HTTP direct-access rows, derived TOGETHER so they cannot
 *  both claim the same address.
 *
 *  A bind pinned to the tailnet address serves the tailnet, not the LAN. Derived
 *  separately, `lanRowState` would print that address under "Local network" with
 *  a "same Wi-Fi" hint (it shows whatever specific address is bound, by design)
 *  and the Tailscale row would then print the very same URL again. Reporting the
 *  LAN as off is the honest half of that: with a tailnet-pinned bind, no LAN
 *  address is served. */
export function directAccessRows(
  gatewayBind: string,
  lanIp: string | null,
  tailnetIp: string | null,
  port: number,
): { lan: LanRowState; tailnetUrl: string | null } {
  const lan = lanRowState(gatewayBind, lanIp, port);
  const tailnetUrl = tailnetHttpUrl(gatewayBind, tailnetIp, port);
  if (lan.kind === 'url' && tailnetUrl !== null && lan.url === tailnetUrl) {
    return { lan: { kind: 'disabled' }, tailnetUrl };
  }
  return { lan, tailnetUrl };
}

/** Which row the Tailscale section shows for this Mac. */
export type TailscaleRowState =
  /** Nothing installed: offer the download. */
  | { kind: 'get' }
  /** Installed but not on a tailnet. `canRun` means we can drive the sign-in
   *  ourselves; without a CLI the user does it in the Tailscale app. */
  | { kind: 'sign-in'; canRun: boolean }
  /** On a tailnet, but nothing is serving HTTPS yet. */
  | { kind: 'expose'; canRun: boolean; magicDnsName: string | null }
  /** On a tailnet AND proven to be serving. */
  | { kind: 'serving'; url: string; canRun: boolean };

/** Pure: derive the Tailscale row from the two independent facts.
 *
 *  Tailnet state decides WHICH row; `cli_available` only decides whether that
 *  row can offer a button. Conflating them is the bug this replaces: a Mac
 *  whose CLI could not be found was reported as signed out, so a machine
 *  sitting on its tailnet was shown a Sign in button that did nothing.
 *
 *  Note the first branch also checks `on_tailnet`: telling someone to install
 *  Tailscale while they are demonstrably using it would be the worst answer
 *  available, so being on a tailnet always wins over an install offer. */
export function tailscaleRowState(ts: TailscaleInfo): TailscaleRowState {
  if (!ts.on_tailnet) {
    return ts.installed ? { kind: 'sign-in', canRun: ts.cli_available } : { kind: 'get' };
  }
  if (ts.serve_url) return { kind: 'serving', url: ts.serve_url, canRun: ts.cli_available };
  return { kind: 'expose', canRun: ts.cli_available, magicDnsName: ts.magic_dns_name };
}

/** Pure: did the device READING this page reach it over the tailnet?
 *
 *  A MagicDNS name resolves only on a device signed in to the tailnet that owns
 *  it, so being served at `<machine>.<tailnet>.ts.net` is proof that Tailscale
 *  is installed and connected right here. Same rule {@link tailscaleRowState}
 *  applies to the Mac: demonstrable tailnet membership always beats an install
 *  offer.
 *
 *  Deliberately NOT extended to a bare `100.64/10` host. That range is real
 *  CGNAT space an ISP can hand to a physical interface, so an address alone
 *  proves nothing without the interface check the Mac-side read does; we cannot
 *  see the phone's interfaces from here. Matching the range in TypeScript would
 *  also fork a predicate that lives once, in `lucidos-tailscale`. An unproven
 *  host keeps the install offer, which is the harmless way to be wrong. */
export function isTailnetHostname(hostname: string): boolean {
  return hostname.toLowerCase().endsWith('.ts.net');
}

/** How far THIS device has got, for the half of the page that needs no Mac.
 *
 *  The address field is `hostname`, never `host`: it carries
 *  `location.hostname`, and a `location.host` would drag a `:port` in behind
 *  the `.ts.net` suffix and read as unproven. */
export type PhoneSetupState =
  /** No proof of a tailnet: offer the install, then the full steps. */
  | { kind: 'install' }
  /** On the tailnet, read in a browser tab: one step left. */
  | { kind: 'add-to-home-screen'; hostname: string }
  /** On the tailnet, running as the installed PWA: nothing left. */
  | { kind: 'ready'; hostname: string };

/** Pure: derive the phone-facing half from how this device got here. Takes the
 *  hostname and the standalone flag as arguments so it is testable without a
 *  `location` or a display-mode media query. */
export function phoneSetupState(hostname: string, standalone: boolean): PhoneSetupState {
  if (!isTailnetHostname(hostname)) return { kind: 'install' };
  return standalone
    ? { kind: 'ready', hostname }
    : { kind: 'add-to-home-screen', hostname };
}

interface MobileAccessInfo {
  connect: ConnectInfo;
  gatewayBind: string;
}

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

/** How to get Tailscale, addressed to whichever device is reading.
 *
 *  Ungated on purpose: this half needs no Tauri IPC, and the person who most
 *  needs it is holding the phone. See the page docs. */
function InstallTailscaleRow({ onPhone }: { onPhone: boolean }) {
  return (
    <div class="list-rows">
      <div class="list-row">
        <div class="list-row-info">
          <div class="title">{onPhone ? 'Install Tailscale on this device' : 'Install Tailscale'}</div>
          <div class="list-row-details">
            {onPhone
              ? 'Then sign in to the same tailnet as your Mac. Tailscale is a VPN, so your phone will ask you to approve a VPN profile.'
              : 'Tailscale is a system VPN, so macOS asks for your approval during install.'}
          </div>
        </div>
        <div class="list-row-actions">
          <button class="action-btn" onClick={openTailscaleDownload}>Get Tailscale</button>
        </div>
      </div>
    </div>
  );
}

/** The Tailscale row for the device reading the page, when that device is not
 *  the Mac.
 *
 *  The install offer is one of three answers, not the only one: a phone that
 *  reached this page over its tailnet has already done that step, and telling it
 *  to install Tailscale is both wrong and the loudest thing on the screen. */
function PhoneTailscaleRow({ state }: { state: PhoneSetupState }) {
  if (state.kind === 'install') return <InstallTailscaleRow onPhone={true} />;
  return (
    <div class="list-rows">
      <div class="list-row">
        <div class="list-row-info">
          <div class="title">Tailscale is connected on this device</div>
          <div class="list-row-details">
            You are reading this over your tailnet, at <strong>{state.hostname}</strong>, so this
            device is already signed in to the same tailnet as your Mac. Nothing to install.
          </div>
        </div>
      </div>
    </div>
  );
}

/** The remaining setup steps for the device reading the page. Each state drops
 *  the steps it can see are already done. */
function PhoneStepsSection({ state }: { state: PhoneSetupState }) {
  const title = state.kind === 'ready' ? 'This phone is set up' : 'Getting Lucidos onto this phone';
  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="mobile-access:phone">{title}</div>
      {state.kind === 'install' && (
        <ol class="settings-section-desc" style={{ paddingLeft: '1.25rem', lineHeight: 1.7 }}>
          <li>Install Tailscale here and sign in to the <strong>same tailnet</strong> as your Mac.</li>
          <li>
            On your Mac, open <strong>Settings → Mobile Access</strong> and copy the{' '}
            <strong>Tailscale</strong> address it shows. Send it to yourself and open it here.
          </li>
          <li>Add that page to your home screen to install Lucidos and turn on push.</li>
        </ol>
      )}
      {state.kind === 'add-to-home-screen' && (
        <p class="settings-section-desc">
          Tailscale is connected and you are already on the right address, so one step is left:
          add this page to your home screen. That installs Lucidos and turns on push
          notifications.
        </p>
      )}
      {state.kind === 'ready' && (
        <p class="settings-section-desc">
          Lucidos is installed here and reaches your Mac over Tailscale, so it works off your home
          network. Nothing left to do.
        </p>
      )}
    </div>
  );
}

/**
 * Mobile Access settings.
 *
 * Two halves, split by whether they need the Mac:
 *
 * - **Machine-side** (Connect URLs, Sign in, Expose) needs the packaged desktop
 *   app: those are Tauri commands with no engine HTTP equivalent.
 * - **The phone-facing half** (what Tailscale buys you, the Tailscale row for
 *   this device, the remaining steps) needs nothing, and is shown everywhere.
 *   This page exists to get the user onto their phone, so gating it behind the
 *   desktop app made it unreachable from the one device it is written for. It
 *   reads its own state off the host this device reached the page on, so a
 *   phone already on the tailnet is never told to install Tailscale.
 *
 * We use `tailscale serve` (tailnet-private HTTPS), never `funnel` (public):
 * the engine has no inbound API auth.
 */
export function MobileAccessPage() {
  // The machine-side half needs the packaged always-on service; a dev Tauri
  // build has no gateway on the stable port, so its connect URLs would be a
  // guess.
  const showMachineHalf = isTauri() && enginePackaged.value;
  // Non-null exactly when the phone-facing half is the one rendering: the Mac
  // has real detection behind `get_connect_info`, this device has the address
  // it is being served on.
  const phone: PhoneSetupState | null = showMachineHalf
    ? null
    : phoneSetupState(window.location.hostname, isStandalone());
  const [info, setInfo] = useState<Loadable<MobileAccessInfo>>({ status: 'not-loaded' });
  const [busy, setBusy] = useState<null | 'up'>(null);
  // The Expose run's state lives in the store, not here: a run can spend minutes
  // waiting for a tailnet approval, it narrates itself on the brand badge and the
  // status toast from anywhere in the app, and a page-local flag would be lost
  // the moment the user navigated away and came back to a still-running setup.
  const serveRunning = tailscaleServeRun.value !== null;
  const [authKey, setAuthKey] = useState('');
  const showLoading = useDelayedLoading(info);

  const reload = useCallback(() => {
    // Not a swallowed error: off the desktop app there is nothing to fetch, so
    // there is no failure to report. Setting a failed state here would blank the
    // phone-facing half, which is exactly the half a phone came for.
    if (!showMachineHalf) return;
    setInfo({ status: 'loading' });
    // The gateway bind decides whether the LAN URL is reachable at all
    // (packaged defaults to loopback-only), so the row cannot render honestly
    // without it — load both together and fail the pane loud on either.
    const connectPromise = getConnectInfo();
    const networkPromise = getNetworkConfig();
    Promise.all([connectPromise, networkPromise])
      .then(([connect, network]) =>
        setInfo({ status: 'loaded', data: { connect, gatewayBind: network.gateway_bind } }),
      )
      .catch(e => setInfo(toFailed(e)));
  }, [showMachineHalf]);

  useEffect(() => { reload(); }, [reload]);

  // Both failure toasts show the error verbatim, with NO action prefix of their
  // own. Every error `mobile.rs` returns already names what failed: the CLI
  // probe's "The Tailscale command-line tool isn't available", the tailnet and
  // MagicDNS preconditions, both "reported success but ..." post-conditions,
  // and `run_checked`'s "tailscale <cmd> failed: <stderr>". Re-framing them here
  // stuttered ("Tailscale serve failed: tailscale serve failed: Error: the CLI
  // for serve and funnel has changed"), which buried the CLI's own advice, and
  // that advice is the whole payload when a syntax change is the cause. Add
  // context to the Rust message instead of a prefix here.
  const onUp = useCallback(async () => {
    setBusy('up');
    try {
      await tailscaleUp(authKey.trim() || undefined);
      setAuthKey('');
      reload();
    } catch (e) {
      showToast(errorDetail(e), 'error');
    } finally {
      setBusy(null);
    }
  }, [authKey, reload]);

  // Expose is narrated by the shared background-activity surface (the spinning
  // brand badge, and the status toast behind it), not by this promise: the run
  // can legitimately wait minutes for a tailnet approval, and every step it
  // passes through arrives as a progress frame. So the outcome toast comes from
  // the terminal frame, and the `catch` here covers only what Rust could not
  // report at all (a rejected invoke, an ACL denial, a dead bridge). Same
  // division of labour as `installAppUpdate`.
  const onServe = useCallback(async () => {
    if (tailscaleServeRun.value) return;
    beginTailscaleServeRun();
    try {
      const url = await tailscaleServe();
      // A still-set run means the terminal frame never arrived: the progress
      // subscription failed, or the whole run finished before it was installed.
      // Settling from the resolved URL is what stops the badge spinning forever
      // over a run that has already succeeded.
      if (tailscaleServeRun.value) {
        applyTailscaleServeProgress({ phase: 'done', url });
      }
      reload();
    } catch (e) {
      // A frame already narrated this (and cleared the run), so say nothing
      // twice. A still-set run means no frame arrived, and then this is the only
      // report there will be.
      if (tailscaleServeRun.value) {
        clearTailscaleServeRun();
        showToast(errorDetail(e), 'error');
      }
    }
  }, [reload]);

  /** The Connect URLs section, plus its loading / failed states. Machine-side. */
  function connectUrlsSection() {
    if (info.status === 'failed') {
      return (
        <div class="settings-section">
          <div class="settings-section-title">Connect URLs</div>
          <LoadableError noun="connect info" error={info.error} />
        </div>
      );
    }
    if (info.status !== 'loaded') {
      if (!showLoading) return null;
      return (
        <div class="settings-section">
          <div class="settings-section-title">Connect URLs</div>
          <div class="empty-state">Loading…</div>
        </div>
      );
    }
    const { connect, gatewayBind } = info.data;
    const { lan, tailnetUrl } = directAccessRows(
      gatewayBind,
      connect.lan_ip,
      connect.tailscale.tailnet_ip,
      connect.port,
    );
    return (
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="mobile-access:urls">Connect URLs</div>
        <p class="settings-section-desc">
          The engine runs as an always-on background service and is reachable at these addresses.
          Open one on another device to use Lucidos from your phone.
        </p>
        <div class="list-rows">
          <UrlRow label="This Mac" url={connect.localhost_url} hint="localhost" />
          {lan.kind === 'url' && (
            <UrlRow
              label="Local network"
              url={lan.url}
              hint="same Wi-Fi · plain HTTP — no PWA install or push (use Tailscale for those)"
            />
          )}
          {lan.kind === 'disabled' && (
            <div class="list-row">
              <div class="list-row-info">
                <div class="title">Local network</div>
                <div class="list-row-details">
                  Off — the gateway only listens on this Mac. Allow LAN access in Network access
                  (applies after a restart).
                </div>
              </div>
              <div class="list-row-actions">
                <button class="action-btn" onClick={() => openSettingsSubview('network-access')}>
                  Network access
                </button>
              </div>
            </div>
          )}
          {lan.kind === 'none' && (
            <div class="list-row">
              <div class="list-row-info">
                <div class="title">Local network</div>
                <div class="list-row-details">No LAN address detected</div>
              </div>
            </div>
          )}
          {/* Only when the gateway actually listens on the tailnet address, and
              only until serving makes the HTTPS row below the better answer. */}
          {tailnetUrl && !connect.tailscale.serve_url && (
            <UrlRow
              label="Tailscale"
              url={tailnetUrl}
              hint="anywhere on your tailnet · plain HTTP, so no PWA install or push yet"
            />
          )}
          {/* Only once serving is PROVEN. Publishing this the moment a MagicDNS
              name resolves advertised an address with nothing listening on it. */}
          {connect.tailscale.serve_url && (
            <UrlRow label="Tailscale" url={connect.tailscale.serve_url} hint="anywhere, HTTPS + push" />
          )}
        </div>
      </div>
    );
  }

  /** The Tailscale action row for this Mac. Machine-side; null until loaded. */
  function tailscaleActionRow() {
    if (info.status !== 'loaded') return null;
    const row = tailscaleRowState(info.data.connect.tailscale);

    if (row.kind === 'get') return <InstallTailscaleRow onPhone={false} />;

    if (row.kind === 'sign-in') {
      return (
        <div class="list-rows">
          <div class="list-row repo-add-form">
            <div class="list-row-info" style={{ gap: '0.5rem' }}>
              <div class="title">Sign in to Tailscale</div>
              <div class="list-row-details">
                {row.canRun
                  ? 'Opens a browser to join your tailnet. Optionally paste a pre-authorized auth key.'
                  : 'Open the Tailscale app in your menu bar and sign in there. This Mac has no Tailscale command-line tool, which is what we would need to do it for you.'}
              </div>
              {row.canRun && (
                <input
                  class="device-name-input"
                  type="text"
                  placeholder="Auth key (optional) — tskey-auth-…"
                  value={authKey}
                  onInput={(e) => setAuthKey((e.target as HTMLInputElement).value)}
                />
              )}
            </div>
            {row.canRun && (
              <div class="list-row-actions">
                <button class="action-btn action-btn-confirm" disabled={busy === 'up'} onClick={onUp}>
                  {busy === 'up' ? 'Signing in…' : 'Sign in'}
                </button>
              </div>
            )}
          </div>
        </div>
      );
    }

    const serving = row.kind === 'serving';
    return (
      <div class="list-rows">
        <div class="list-row">
          <div class="list-row-info">
            <div class="title">
              {serving ? 'Serving the engine over Tailscale' : 'Expose the engine over Tailscale'}
            </div>
            <div class="list-row-details">
              {serving
                ? <>Reachable at <strong>{row.url}</strong> with an auto-renewed cert.</>
                : row.canRun
                  ? 'Sets up tailnet HTTPS for the engine, which is what enables the installable PWA and push. Setup runs in the background and reports on the Lucidos badge, and it may ask you to enable Serve for your tailnet.'
                  : 'Your phone can already reach the plain-HTTP address above. HTTPS is what adds the installable PWA and push, and it needs `tailscale serve`. Install the Tailscale command-line tool to set it up: use Install CLI in the Tailscale app, or `brew install tailscale`.'}
            </div>
          </div>
          {row.canRun && (
            <div class="list-row-actions">
              <button class="action-btn action-btn-confirm" disabled={serveRunning} onClick={onServe}>
                {serveRunning ? 'Setting up…' : (serving ? 'Re-apply' : 'Expose')}
              </button>
            </div>
          )}
        </div>
      </div>
    );
  }

  return (
    <>
      {showMachineHalf && connectUrlsSection()}

      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="mobile-access:tailscale">Tailscale (recommended)</div>
        <p class="settings-section-desc">
          Tailscale gives your phone a private, encrypted HTTPS link to your Mac that works off
          your home network, which is what enables the installable PWA and push notifications. It
          stays tailnet-private (we use <code>serve</code>, not <code>funnel</code>), so the engine
          is never exposed to the public internet.
        </p>
        {phone ? <PhoneTailscaleRow state={phone} /> : tailscaleActionRow()}
      </div>

      {phone ? <PhoneStepsSection state={phone} /> : (
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="mobile-access:phone">
            On your phone
          </div>
          <ol class="settings-section-desc" style={{ paddingLeft: '1.25rem', lineHeight: 1.7 }}>
            <li>Install the Tailscale app and sign in to the <strong>same tailnet</strong> as this Mac.</li>
            <li>Open the <strong>Tailscale</strong> URL above (copy it / send it to yourself).</li>
            <li>Add it to your home screen to install the Lucidos PWA and enable push.</li>
          </ol>
        </div>
      )}
    </>
  );
}
