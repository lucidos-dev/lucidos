import type { ComponentChildren } from 'preact';
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
import type { NetworkConfigResponse } from '../../api/types';
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
  return tailnetIsServed(gatewayBind, tailnetIp) ? `http://${tailnetIp}:${port}` : null;
}

/** Pure: does this bind actually put a listener on the tailnet address?
 *
 *  Extracted from {@link tailnetHttpUrl} so the prose that DESCRIBES tailnet
 *  reachability cannot drift from the rule that decides whether to print a URL
 *  for it. Being on a tailnet and being reachable on it are different facts,
 *  and a row that conflates them advertises a dead address: under the packaged
 *  loopback default nothing off this machine can connect, however healthy
 *  Tailscale is. */
export function tailnetIsServed(gatewayBind: string, tailnetIp: string): boolean {
  const bind = gatewayBind.trim();
  return bind === 'all' || bind === tailnetIp;
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

/** What we can say about the machine running the engine, from the facts a
 *  browser can reach.
 *
 *  This is **concern 1** of the two this page answers, and it is a property of
 *  the machine, never of the reader. `unknown` is a real state and not a
 *  synonym for `no-tailnet`: claiming a machine is off the tailnet because a
 *  fetch has not landed yet is a claim we cannot support. */
export type HostTailnetState =
  | { kind: 'on-tailnet'; ip: string }
  | { kind: 'no-tailnet' }
  | { kind: 'unknown' };

/** Pure: derive the host's tailnet state from `GET /api/v1/network-config`.
 *
 *  `detected_tailscale_ip` is the engine's own reading of its interface list
 *  (`lucidos_tailscale::tailnet_ipv4`), which requires BOTH a Tailscale
 *  interface and the `100.64/10` range. So it is an interface-checked fact
 *  arriving over plain HTTP, which is what lets this section render in a
 *  browser with no Tauri bridge at all. */
export function hostTailnetState(
  config: Loadable<{ detected_tailscale_ip: string | null }>,
): HostTailnetState {
  if (config.status !== 'loaded') return { kind: 'unknown' };
  const ip = config.data.detected_tailscale_ip;
  return ip ? { kind: 'on-tailnet', ip } : { kind: 'no-tailnet' };
}

/** Pure: is this a MagicDNS name?
 *
 *  One of the three proofs behind {@link deviceSetupState}. A MagicDNS name
 *  resolves only on a device signed in to the tailnet that owns it, so being
 *  served at `<machine>.<tailnet>.ts.net` is proof that Tailscale is installed
 *  and connected right here.
 *
 *  Deliberately does NOT match a bare `100.64/10` host. That range is real
 *  CGNAT space an ISP can hand to a physical interface, so the range alone
 *  proves nothing, and re-implementing the range test here would fork a
 *  predicate that lives once, in `lucidos-tailscale`. The tailnet-address proof
 *  is an equality check against what the engine reported, not a range match:
 *  see {@link deviceSetupState}. */
export function isTailnetHostname(hostname: string): boolean {
  return hostname.trim().toLowerCase().endsWith('.ts.net');
}

/** Pure: was this page served by the very machine reading it?
 *
 *  Loopback is the one sound implication between the page's two concerns: a
 *  request that never left the machine was answered by the engine host, so the
 *  reader IS the host and concern 1 is the whole answer for it. Covers all of
 *  `127.0.0.0/8` and the RFC 6761 `.localhost` tree, the latter because Tauri
 *  serves its bundled assets from `tauri.localhost` off macOS. */
export function isLoopbackHostname(hostname: string): boolean {
  const host = hostname.trim().toLowerCase().replace(/^\[|\]$/g, '');
  if (host === 'localhost' || host.endsWith('.localhost')) return true;
  if (host === '::1') return true;
  const v4 = /^(\d{1,3})\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.exec(host);
  return v4 !== null && v4[1] === '127';
}

/** How far THIS device has got. **Concern 2** of the two, and a property of the
 *  reader alone.
 *
 *  The address field is `hostname`, never `host`: it carries
 *  `location.hostname`, and a `location.host` would drag a `:port` in behind
 *  the `.ts.net` suffix and read as unproven. */
export type DeviceSetupState =
  /** The reader is the engine host, so there is no tailnet to join here. */
  | { kind: 'same-machine' }
  /** No proof of a tailnet: offer the Tailscale install, then the full steps. */
  | { kind: 'join-tailnet' }
  /** On the tailnet, but over plain HTTP, so the browser will not install
   *  anything here however much we ask it to. The remaining step belongs to the
   *  machine, not to this device: `tailscale serve`. */
  | { kind: 'needs-https'; hostname: string }
  /** On the tailnet, on a secure origin, read in a browser tab: one step left. */
  | { kind: 'install-app'; hostname: string }
  /** On the tailnet, running as the installed PWA: nothing left. */
  | { kind: 'ready'; hostname: string };

/** Pure: derive the reading device's state from how it got here.
 *
 *  Three proofs of tailnet membership, each sound on its own:
 *
 *  1. **Loopback**: the reader is the host (see {@link isLoopbackHostname}),
 *     which short-circuits everything below. Whatever the host's tailnet state
 *     is, it is reported by concern 1 and not by an install offer here.
 *  2. **A MagicDNS name** ({@link isTailnetHostname}).
 *  3. **The host's own tailnet address**, matched exactly. The engine read that
 *     address off a Tailscale interface, and this request arrived at it, so
 *     this device is on that tailnet. This is the proof the page was missing:
 *     a gateway bound to its tailnet address serves every remote device at a
 *     bare `100.x` host, and every one of them was being told to install
 *     Tailscale.
 *
 *  Being on the tailnet is NOT the last question. `secureContext` decides which
 *  step remains, because the installable app and push are gated on a secure
 *  origin and a tailnet address over plain `http://` is not one. Telling such a
 *  reader to install the app names a control their browser will never offer:
 *  the actual remaining work is `tailscale serve` on the machine, which is
 *  concern 1's business. (A `standalone` reader is on a secure origin by
 *  construction, since a service worker cannot be registered otherwise, so that
 *  branch is tested first and needs no flag of its own.)
 *
 *  Every input is passed in, so this is testable without a `location`, a
 *  display-mode media query, `isSecureContext`, or a live engine. A `null` host
 *  address costs proof 3 and nothing else, which is why a failed
 *  `network-config` degrades this rather than breaking it. */
export function deviceSetupState(device: {
  hostname: string;
  standalone: boolean;
  secureContext: boolean;
  hostTailnetIp: string | null;
}): DeviceSetupState {
  const hostname = device.hostname.trim();
  if (isLoopbackHostname(hostname)) return { kind: 'same-machine' };
  const onTailnet =
    isTailnetHostname(hostname) ||
    (device.hostTailnetIp !== null && hostname === device.hostTailnetIp.trim());
  if (!onTailnet) return { kind: 'join-tailnet' };
  if (device.standalone) return { kind: 'ready', hostname };
  return device.secureContext
    ? { kind: 'install-app', hostname }
    : { kind: 'needs-https', hostname };
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
        <div class="list-row-details list-row-details-prose">
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

/** Is the device reading this a handset, i.e. one the "home screen" and "phone"
 *  wording is actually true of?
 *
 *  A desktop browser installs a PWA from the address bar and has no home
 *  screen, so calling it a phone is both wrong and the loudest thing on the
 *  page. Matched on the same two flags {@link tailscaleDownloadUrl} routes on,
 *  which is the only device question this file already knew how to answer. */
function isHandset(): boolean {
  return isIOS() || isAndroid();
}

/** How to get Tailscale on the device reading this.
 *
 *  Ungated on purpose: it needs no Tauri IPC, and the person who most needs it
 *  is holding the device that lacks Tailscale. `onHost` distinguishes the two
 *  callers, because the sentence differs: installing on the engine host is a
 *  step towards serving Lucidos, installing on a client is a step towards
 *  reaching it. */
function InstallTailscaleRow({ onHost }: { onHost: boolean }) {
  const handset = isHandset();
  return (
    <div class="list-rows">
      <div class="list-row">
        <div class="list-row-info">
          <div class="title">
            {onHost ? 'Install Tailscale on this machine' : 'Install Tailscale on this device'}
          </div>
          <div class="list-row-details list-row-details-prose">
            {onHost
              ? 'Tailscale is a system VPN, so your OS asks for your approval during install. Sign in afterwards to put this machine on a tailnet.'
              : handset
                ? 'Then sign in to the same tailnet as the machine running Lucidos. Tailscale is a VPN, so your phone will ask you to approve a VPN profile.'
                : 'Then sign in to the same tailnet as the machine running Lucidos. Tailscale is a system VPN, so your OS asks for your approval during install.'}
          </div>
        </div>
        <div class="list-row-actions">
          <button class="action-btn" onClick={openTailscaleDownload}>Get Tailscale</button>
        </div>
      </div>
    </div>
  );
}

/** One plain row of prose, which is most of what both concerns render.
 *
 *  The details slot is a sentence, so it takes `list-row-details-prose`: the
 *  base class alone is a flex row of fields, which would make each inline
 *  `<strong>`/`<code>` its own flex item and strand the punctuation after it. */
function InfoRow({ title, children }: { title: string; children: ComponentChildren }) {
  return (
    <div class="list-rows">
      <div class="list-row">
        <div class="list-row-info">
          <div class="title">{title}</div>
          <div class="list-row-details list-row-details-prose">{children}</div>
        </div>
      </div>
    </div>
  );
}

/** **Concern 1** as a browser can see it: is the machine running the engine on
 *  a tailnet?
 *
 *  Reporting only. Every action that could change this answer (`tailscale up`,
 *  `tailscale serve`) is a native command with no HTTP equivalent, so the
 *  packaged app renders the action row instead of this one. The single
 *  exception is the install offer, which is a link rather than an IPC call, and
 *  is therefore worth showing to a reader who IS the host and can act on it.
 *  Offering it to a remote reader would be a button aimed at the wrong machine.
 *
 *  Says only what `detected_tailscale_ip` supports. Serve state and the MagicDNS
 *  name are not visible from here, so this row claims neither. `reachable` is a
 *  SEPARATE fact from `on-tailnet` and must stay one: holding a tailnet address
 *  says nothing about whether anything is listening on it. */
function HostTailnetRow({
  state,
  readerIsHost,
  reachable,
}: {
  state: HostTailnetState;
  readerIsHost: boolean;
  reachable: boolean;
}) {
  if (state.kind === 'unknown') {
    return <InfoRow title="Checking this machine">Reading its Tailscale state.</InfoRow>;
  }
  if (state.kind === 'on-tailnet') {
    return (
      <InfoRow title="This machine is on a tailnet">
        The machine running Lucidos holds the tailnet address <strong>{state.ip}</strong>.{' '}
        {reachable
          ? 'It is listening on that address, so any device signed in to the same tailnet can reach it there over plain HTTP.'
          : 'It is NOT listening on that address though, so nothing off this machine can connect to it there yet: allow it in Network access, which takes effect after a restart.'}{' '}
        Adding HTTPS with <code>tailscale serve</code> is what enables the installable app and
        push. That needs the Tailscale command-line tool, and it works whatever the address above
        says, since it proxies from this machine to <code>127.0.0.1</code>.
      </InfoRow>
    );
  }
  if (readerIsHost) return <InstallTailscaleRow onHost={true} />;
  return (
    <InfoRow title="This machine is not on a tailnet">
      The machine running Lucidos has no tailnet address, so no device can reach it over Tailscale.
      Install Tailscale there and sign it in to your tailnet.
    </InfoRow>
  );
}

/** **Concern 2**: has the device reading this joined the tailnet?
 *
 *  The install offer is one of four answers, not the only one. A device that
 *  reached this page over its tailnet has already done that step, and a device
 *  reading over loopback IS the engine host, for which the question does not
 *  arise at all. */
function DeviceTailscaleRow({ state }: { state: DeviceSetupState }) {
  if (state.kind === 'join-tailnet') return <InstallTailscaleRow onHost={false} />;
  if (state.kind === 'same-machine') {
    return (
      <InfoRow title="You are reading this on the machine that runs Lucidos">
        There is no tailnet to join here: this is the machine itself. What matters is the section
        above. Once this machine is on a tailnet, open its address on another device to carry on
        there.
      </InfoRow>
    );
  }
  return (
    <InfoRow title="Tailscale is connected on this device">
      You are reading this over your tailnet, at <strong>{state.hostname}</strong>, so this device
      is already signed in to the same tailnet as the machine running Lucidos. Nothing to install.
    </InfoRow>
  );
}

/** The remaining setup steps for the device reading the page. Each state drops
 *  the steps it can see are already done. */
function DeviceStepsSection({ state }: { state: DeviceSetupState }) {
  const handset = isHandset();
  // Naming the destination beats naming a device class we would then get wrong:
  // a desktop browser installs a PWA from the address bar and has no home
  // screen, and this page is read from both.
  const install = handset
    ? 'Add it to your home screen to install Lucidos and turn on push notifications.'
    : 'Install Lucidos from your browser (the install control in the address bar) to turn on push notifications.';
  if (state.kind === 'same-machine') {
    return (
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="mobile-access:steps">
          Getting Lucidos onto another device
        </div>
        <ol class="settings-section-desc" style={{ paddingLeft: '1.25rem', lineHeight: 1.7 }}>
          <li>Put this machine on a tailnet, and set up HTTPS for it with <code>tailscale serve</code>.</li>
          <li>
            Install Tailscale on the other device and sign in to the <strong>same tailnet</strong>.
          </li>
          <li>Open this machine's Tailscale address there, and install Lucidos from that page.</li>
        </ol>
      </div>
    );
  }
  const title = state.kind === 'ready' ? 'This device is set up' : 'Getting Lucidos onto this device';
  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="mobile-access:steps">{title}</div>
      {state.kind === 'needs-https' && (
        <p class="settings-section-desc">
          Tailscale is connected and you are on the right address, but this one is plain{' '}
          <code>http://</code>. Browsers gate the installable app and push notifications on a
          secure origin, so neither is available here and no amount of asking will offer them.
          The remaining step is on the machine, not on this device: set up HTTPS for it with{' '}
          <code>tailscale serve</code>, then open the <code>https://</code> address it gives you.
          Reading and chatting work fine at this address meanwhile.
        </p>
      )}
      {state.kind === 'join-tailnet' && (
        <ol class="settings-section-desc" style={{ paddingLeft: '1.25rem', lineHeight: 1.7 }}>
          <li>
            Install Tailscale here and sign in to the <strong>same tailnet</strong> as the machine
            running Lucidos.
          </li>
          <li>
            On that machine, open <strong>Settings → Mobile Access</strong> and copy the{' '}
            <strong>Tailscale</strong> address it shows. Send it to yourself and open it here.
          </li>
          <li>{install}</li>
        </ol>
      )}
      {state.kind === 'install-app' && (
        <p class="settings-section-desc">
          Tailscale is connected and you are already on the right address, so one step is left.{' '}
          {install}
        </p>
      )}
      {state.kind === 'ready' && (
        <p class="settings-section-desc">
          Lucidos is installed here and reaches the machine it runs on over Tailscale, so it works
          off your home network. Nothing left to do.
        </p>
      )}
    </div>
  );
}

/**
 * Mobile Access settings.
 *
 * **Two concerns, and they must not be muddled.** The page answers both, always,
 * in this order:
 *
 * 1. **Is the machine running the engine on a tailnet?** A property of the
 *    machine. Read over plain HTTP from `GET /api/v1/network-config`
 *    (`detected_tailscale_ip`), so it renders in any browser; upgraded on the
 *    packaged desktop app to the full `get_connect_info` picture, which is the
 *    only place the Sign in / Expose actions can exist.
 * 2. **Has the device reading this joined that tailnet?** A property of the
 *    reader, derived from the address it was served on. See
 *    {@link deviceSetupState} for the three proofs.
 *
 * The page used to pick ONE of these by platform (`isTauri() &&
 * enginePackaged`), which is the bug this structure replaces: a browser saw
 * concern 2 alone, so a machine whose gateway is bound to its own tailnet
 * address told every reader to install Tailscale, under a heading calling their
 * laptop a phone, while the section that would have shown them the address to
 * use was the one being suppressed.
 *
 * What varies by platform is how much each section can SAY, never whether it
 * appears. Actions stay gated on the native bridge, because `tailscale up` and
 * `tailscale serve` have no HTTP equivalent; reporting is not gated, because it
 * never needed to be.
 *
 * We use `tailscale serve` (tailnet-private HTTPS), never `funnel` (public):
 * the engine has no inbound API auth.
 */
export function MobileAccessPage() {
  // The Connect URLs and the Tailscale ACTIONS need the packaged always-on
  // service; a dev Tauri build has no gateway on the stable port, so its
  // connect URLs would be a guess. Reporting is not gated on this.
  const showMachineHalf = isTauri() && enginePackaged.value;
  const [connectInfo, setConnectInfo] = useState<Loadable<ConnectInfo>>({ status: 'not-loaded' });
  const [netConfig, setNetConfig] = useState<Loadable<NetworkConfigResponse>>({ status: 'not-loaded' });
  const [busy, setBusy] = useState<null | 'up'>(null);
  // The Expose run's state lives in the store, not here: a run can spend minutes
  // waiting for a tailnet approval, it narrates itself on the brand badge and the
  // status toast from anywhere in the app, and a page-local flag would be lost
  // the moment the user navigated away and came back to a still-running setup.
  const serveRunning = tailscaleServeRun.value !== null;
  const [authKey, setAuthKey] = useState('');
  const showLoading = useDelayedLoading(connectInfo);

  // Concern 1, from whichever source this platform has. Concern 2 reads the
  // host's address as ONE of its three proofs and is otherwise independent of
  // it, so a `network-config` that never landed costs that one proof and leaves
  // the rest of the derivation intact.
  const host: HostTailnetState = showMachineHalf && connectInfo.status === 'loaded'
    ? (connectInfo.data.tailscale.tailnet_ip
        ? { kind: 'on-tailnet', ip: connectInfo.data.tailscale.tailnet_ip }
        : { kind: 'no-tailnet' })
    : hostTailnetState(netConfig);
  const device: DeviceSetupState = deviceSetupState({
    hostname: window.location.hostname,
    standalone: isStandalone(),
    secureContext: window.isSecureContext,
    hostTailnetIp: host.kind === 'on-tailnet' ? host.ip : null,
  });
  // Reachability is NOT implied by tailnet membership: under a loopback bind
  // nothing off this machine can connect, however healthy Tailscale is. Two
  // ways to know it is served, and the first outranks the second, because a
  // bind change only takes effect on restart and the config can therefore
  // disagree with the live socket: this very page arrived at that address, or
  // the configured bind covers it (the same rule `tailnetHttpUrl` applies
  // before printing a URL).
  const tailnetReachable =
    host.kind === 'on-tailnet' &&
    (window.location.hostname === host.ip ||
      (netConfig.status === 'loaded' && tailnetIsServed(netConfig.data.gateway_bind, host.ip)));

  const reload = useCallback(() => {
    // Fetched on EVERY platform: this is the only reading of concern 1 a
    // browser has, and it is also what proves a remote device is on the tailnet
    // when it reached us at a bare `100.x` address.
    setNetConfig({ status: 'loading' });
    getNetworkConfig().then(
      (data) => setNetConfig({ status: 'loaded', data }),
      (e) => setNetConfig(toFailed(e)),
    );
    // Not a swallowed error: off the desktop app there is nothing to fetch
    // here, so there is no failure to report. Setting a failed state would
    // render the error card for a bridge this platform was never going to have.
    if (!showMachineHalf) return;
    setConnectInfo({ status: 'loading' });
    getConnectInfo().then(
      (data) => setConnectInfo({ status: 'loaded', data }),
      (e) => setConnectInfo(toFailed(e)),
    );
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

  /** The Connect URLs section, plus its loading / failed states. Machine-side.
   *
   *  Needs BOTH loads: the gateway bind decides whether a LAN or tailnet URL is
   *  reachable at all (packaged defaults to loopback-only), so the rows cannot
   *  render honestly without it. Hence one failure branch covering either, which
   *  is what the single combined fetch used to buy. */
  function connectUrlsSection() {
    const failure =
      connectInfo.status === 'failed' ? connectInfo.error
      : netConfig.status === 'failed' ? netConfig.error
      : null;
    if (failure !== null) {
      return (
        <div class="settings-section">
          <div class="settings-section-title">Connect URLs</div>
          <LoadableError noun="connect info" error={failure} />
        </div>
      );
    }
    if (connectInfo.status !== 'loaded' || netConfig.status !== 'loaded') {
      if (!showLoading) return null;
      return (
        <div class="settings-section">
          <div class="settings-section-title">Connect URLs</div>
          <div class="empty-state">Loading…</div>
        </div>
      );
    }
    const connect = connectInfo.data;
    const { lan, tailnetUrl } = directAccessRows(
      netConfig.data.gateway_bind,
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
                <div class="list-row-details list-row-details-prose">
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
                <div class="list-row-details list-row-details-prose">No LAN address detected</div>
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

  /** Concern 1's body, whichever source this platform reads it from.
   *
   *  A FAILED load is reported, never rendered as a permanent "Checking this
   *  machine": `hostTailnetState` folds failed into `unknown` because there is
   *  no honest tailnet answer to give, but "we could not ask" and "we have not
   *  asked yet" read identically on screen, and the first is a swallowed error
   *  (`.claude/rules/frontend.md` § No Hidden Errors). Same shape both sides,
   *  since both sides can fail. */
  function hostSection() {
    if (showMachineHalf) return tailscaleActionRow();
    if (netConfig.status === 'failed') {
      return <LoadableError noun="this machine's Tailscale state" error={netConfig.error} />;
    }
    return (
      <HostTailnetRow
        state={host}
        readerIsHost={device.kind === 'same-machine'}
        reachable={tailnetReachable}
      />
    );
  }

  /** Concern 1 with its actions, for the packaged desktop app. Falls back to
   *  {@link HostTailnetRow} until the bridge answers, so the section is never
   *  empty while `get_connect_info` runs its few seconds of probes. */
  function tailscaleActionRow() {
    if (connectInfo.status === 'failed') {
      return <LoadableError noun="this machine's Tailscale state" error={connectInfo.error} />;
    }
    if (connectInfo.status !== 'loaded') {
      // `unknown` renders neither claim, so `reachable` cannot be read here.
      return <HostTailnetRow state={{ kind: 'unknown' }} readerIsHost={true} reachable={false} />;
    }
    const row = tailscaleRowState(connectInfo.data.tailscale);

    // Reader and host are the same machine here by construction: this branch is
    // the packaged desktop app, so the install offer is one it can act on.
    if (row.kind === 'get') return <InstallTailscaleRow onHost={true} />;

    if (row.kind === 'sign-in') {
      return (
        <div class="list-rows">
          <div class="list-row repo-add-form">
            <div class="list-row-info" style={{ gap: '0.5rem' }}>
              <div class="title">Sign in to Tailscale</div>
              <div class="list-row-details list-row-details-prose">
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
            <div class="list-row-details list-row-details-prose">
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

  // Two sections, both always rendered, in the order the setup happens: the
  // machine has to be on a tailnet before joining one buys a device anything.
  // Which of the two the reader can act on is a separate question from which of
  // them is TRUE, and conflating those is what this page used to do.
  return (
    <>
      {showMachineHalf && connectUrlsSection()}

      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="mobile-access:tailscale">Tailscale (recommended)</div>
        <p class="settings-section-desc">
          Tailscale gives your other devices a private, encrypted HTTPS link to the machine
          running Lucidos that works off your home network, which is what enables the installable
          PWA and push notifications. It stays tailnet-private (we use <code>serve</code>, not{' '}
          <code>funnel</code>), so the engine is never exposed to the public internet. Two things
          have to be true, and they are independent: the machine is on a tailnet, and the device
          you want to use has joined it.
        </p>
      </div>

      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="mobile-access:machine">
          1. The machine running Lucidos
        </div>
        {hostSection()}
      </div>

      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="mobile-access:device">
          2. This device
        </div>
        <DeviceTailscaleRow state={device} />
      </div>

      <DeviceStepsSection state={device} />
    </>
  );
}
