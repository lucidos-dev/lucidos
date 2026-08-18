import type { ComponentChildren } from 'preact';
import { useState, useEffect, useCallback } from 'preact/hooks';
import { showToast, enginePackaged, tailscaleServeRun, settingsScrollTarget } from '../../store/store';
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
import { getNetworkConfig, getTailnetStatus } from '../../api/client';
import { SCOPE_PATH, WORKSPACE_ID } from '../../utils/basePath';
import { Explainer } from '../shared/Explainer';
import type { NetworkConfigResponse, TailnetStatusResponse } from '../../api/types';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
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
 *  caveat as the Network access section below it on this page. */
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

/** Pure: the Local network row, silenced when the address it would print is the
 *  tailnet's.
 *
 *  A bind pinned to the tailnet address serves the tailnet, not the LAN.
 *  `lanRowState` shows whatever specific address is bound, by design. So on its
 *  own it prints that address under "Local network" with a "same Wi-Fi" hint,
 *  beside a Tailscale row carrying the very same URL. Reporting the LAN as off
 *  is the honest half: with a tailnet-pinned bind, no LAN address is served.
 *
 *  Returns the LAN row alone. It used to hand back the tailnet URL beside it,
 *  from when both rows were derived here. That ended when the Tailscale row
 *  moved to {@link tailnetConnectRows}, which any browser can reach. Two
 *  functions each offering "the tailnet URL" is one more than the page prints. */
export function lanRowAvoidingTailnet(
  gatewayBind: string,
  lanIp: string | null,
  tailnetIp: string | null,
  port: number,
): LanRowState {
  const lan = lanRowState(gatewayBind, lanIp, port);
  const tailnetUrl = tailnetHttpUrl(gatewayBind, tailnetIp, port);
  if (lan.kind === 'url' && tailnetUrl !== null && lan.url === tailnetUrl) {
    return { kind: 'disabled' };
  }
  return lan;
}

/** Pure: the URL that reaches THIS workspace at `origin`.
 *
 *  A workspace is addressed by the first path segment of the gateway origin
 *  (ADR 0014). A bare origin therefore reaches the gateway ROOT, which
 *  307-redirects to the sole workspace or to the picker. On an install with
 *  more than one workspace that is the wrong answer, and it is what every row
 *  on this page used to print.
 *
 *  `scope` is `SCOPE_PATH`, taken from the `<base href>` the engine stamps, so
 *  this stays slug-agnostic: `/<slug>/` behind the gateway, and `/` on a direct
 *  engine port where there is no prefix to add. */
export function workspaceUrlAt(origin: string, scope: string): string {
  const base = origin.replace(/\/+$/, '');
  return `${base}${scope.startsWith('/') ? scope : `/${scope}`}`;
}

/** Pure: the reader's own origin with the hostname swapped for `host`.
 *
 *  The whole derivation of a remote URL from a local page. Keep the scheme,
 *  keep the port, change only the host. Whatever server answered this page at
 *  `<host>:<port>` is the same process answering at `<other>:<port>`, when it
 *  listens there. The scheme is kept rather than assumed, because the dev
 *  gateway speaks TLS and a composed `http://` would be dead.
 *
 *  `port` is `location.port`, which is `''` on a default port. The origin then
 *  carries no `:port` either. */
export function originAtHost(here: { protocol: string; port: string }, host: string): string {
  return `${here.protocol}//${host}${here.port ? `:${here.port}` : ''}`;
}

/** Pure: the network bind of whichever process served this page.
 *
 *  Behind the gateway this origin IS the gateway, so its bind decides. On a
 *  direct engine port the origin is the engine, which follows the gateway bind
 *  only while `inherit` is on. Reading `gateway_bind` unconditionally, which
 *  this replaces, reports a direct-port page's reachability from a bind that
 *  governs a different process. */
export function servingBind(
  config: { engine_bind: string; inherit: boolean; gateway_bind: string },
  behindGateway: boolean,
): string {
  if (behindGateway) return config.gateway_bind;
  return config.inherit ? config.gateway_bind : config.engine_bind;
}

/** One Connect URLs row: a copyable address plus the sentence qualifying it. */
export type ConnectUrlRow = { label: string; url: string; hint: string };

/** Pure: the tailnet rows of Connect URLs, from facts any browser can hold.
 *
 *  This is what ungates the section off the packaged desktop app. Every input
 *  comes from the reader's own `location`, `GET /api/v1/network-config` or
 *  `GET /api/v1/tailnet-status`.
 *
 *  A verified HTTPS URL wins outright, and is printed verbatim. The engine sets
 *  it only after a request to it came back from that same engine. It is NOT
 *  bind-gated, because `serve` proxies to `127.0.0.1` and so survives the
 *  packaged loopback default. The plain-HTTP row then yields to it, being the
 *  worse answer to the same question.
 *
 *  Otherwise the plain-HTTP row prints only when the address is served, since a
 *  dead copyable URL is the bug this page was built around. It prefers the
 *  MagicDNS name, which resolves to that same address for any device on the
 *  tailnet and is what a person can retype. */
export function tailnetConnectRows(input: {
  scope: string;
  here: { protocol: string; hostname: string; port: string };
  tailnetIp: string | null;
  magicDnsName: string | null;
  workspaceServeUrl: string | null;
  bind: string;
}): ConnectUrlRow[] {
  if (input.workspaceServeUrl) {
    // Always HTTPS: the engine builds this one, and only from `https://`.
    return [{ label: 'Tailscale', url: input.workspaceServeUrl, hint: tailnetHint(true) }];
  }
  const ip = input.tailnetIp;
  if (!ip || !tailnetServesThisReader(input.bind, ip, input.here.hostname)) return [];
  return [
    {
      label: 'Tailscale',
      url: workspaceUrlAt(originAtHost(input.here, input.magicDnsName ?? ip), input.scope),
      hint: tailnetHint(input.here.protocol === 'https:'),
    },
  ];
}

/** The sentence under a tailnet URL, by what that address can actually do.
 *
 *  Browsers gate the installable app and push on a secure origin, so the scheme
 *  is the whole difference. The hint must READ it rather than assume it from
 *  which branch above produced the row: this row keeps the reader's own scheme,
 *  and two ordinary setups reach it over HTTPS. The dev gateway holds a TLS cert
 *  of its own, and a verification that timed out drops a serve-fronted reader
 *  here. Assuming plain HTTP told both of them that push was unavailable at an
 *  origin offering it, under an `https://` address saying otherwise. */
function tailnetHint(secure: boolean): string {
  return secure
    ? 'anywhere on your tailnet · HTTPS, so the app install and push work here'
    : 'anywhere on your tailnet · plain HTTP, so no app install or push yet';
}

/** Pure: is the gateway accepting connections AT the tailnet address itself?
 *
 *  One specific socket: `<tailnet ip>:<gateway port>`, over plain HTTP. Direct
 *  evidence outranks the stored bind, and has to. A bind change takes effect
 *  only on restart, so the config can disagree with the live socket. So the two
 *  ways to know are that this page arrived at that very address, or that the
 *  serving process's bind covers it. */
export function tailnetAddressIsServed(
  bind: string,
  tailnetIp: string,
  readerHostname: string,
): boolean {
  return readerHostname.trim() === tailnetIp.trim() || tailnetIsServed(bind, tailnetIp);
}

/** Pure: is there a live tailnet address to print a URL at for THIS reader?
 *
 *  Wider than {@link tailnetAddressIsServed} by exactly one disjunct, and the
 *  two must not be merged. A row prints the reader's own origin with the host
 *  swapped. So a reader who arrived over a MagicDNS name is handed the address
 *  they are already on, which is live by construction.
 *
 *  That arrival is NOT evidence for the narrower question. It proves 443 is
 *  fronted by `serve` and says nothing about the gateway port on the tailnet
 *  address. Letting it answer both told a phone on the recommended setup that
 *  the plain-HTTP address was live. That setup is `serve` plus the packaged
 *  loopback default, where the page had correctly pointed at Network access. */
export function tailnetServesThisReader(
  bind: string,
  tailnetIp: string,
  readerHostname: string,
): boolean {
  return (
    tailnetAddressIsServed(bind, tailnetIp, readerHostname) ||
    isTailnetHostname(readerHostname.trim())
  );
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
        <div class="settings-section-title" data-search-anchor="access:steps">
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
      <div class="settings-section-title" data-search-anchor="access:steps">{title}</div>
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
          On that machine, open <strong>Settings → Access</strong> and copy the{' '}
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
 * The mobile-access half of Settings -> Access (the network bind is the other
 * half, rendered after this page by `accessSection`).
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
  // What still needs the packaged always-on service: the Tailscale ACTIONS, and
  // the This Mac / Local network rows. Those two addresses come from the bridge,
  // and a dev Tauri build has no gateway on the stable port, so they would be a
  // guess. Connect URLs itself is not gated, and neither is any other reporting.
  const showMachineHalf = isTauri() && enginePackaged.value;
  const [connectInfo, setConnectInfo] = useState<Loadable<ConnectInfo>>({ status: 'not-loaded' });
  const [netConfig, setNetConfig] = useState<Loadable<NetworkConfigResponse>>({ status: 'not-loaded' });
  // The MagicDNS name and this workspace's verified HTTPS URL, on every
  // platform. Its own request rather than fields on `network-config`, which the
  // bind editor below also fetches and which must stay a cheap local read.
  const [tailnetStatus, setTailnetStatus] = useState<Loadable<TailnetStatusResponse>>({ status: 'not-loaded' });
  const [busy, setBusy] = useState<null | 'up'>(null);
  // The Expose run's state lives in the store, not here: a run can spend minutes
  // waiting for a tailnet approval, it narrates itself on the brand badge and the
  // status toast from anywhere in the app, and a page-local flag would be lost
  // the moment the user navigated away and came back to a still-running setup.
  const serveRunning = tailscaleServeRun.value !== null;
  const [authKey, setAuthKey] = useState('');
  // Connect URLs needs the two HTTP reads on every platform, and the bridge only
  // where there is one. A `useDelayedLoading(connectInfo)` would sit at
  // `not-loaded` forever in a browser, which is indistinguishable from a slow
  // load and would hold the loader up permanently.
  const connectUrlsReady =
    netConfig.status === 'loaded' &&
    tailnetStatus.status === 'loaded' &&
    (!showMachineHalf || connectInfo.status === 'loaded');
  const showLoading = useDelayedFlag(!connectUrlsReady);

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
  // nothing off this machine can connect, however healthy Tailscale is. This
  // prose claims the plain-HTTP address specifically, so it takes the NARROWER
  // predicate and must not borrow the URL row's. The bind it weighs is the
  // SERVING process's: on a direct engine port the origin is the engine, and
  // `gateway_bind` governs somebody else.
  // An unloaded config contributes NO bind rather than blocking the answer: the
  // empty string is served by nothing, so direct evidence still stands alone.
  const bind =
    netConfig.status === 'loaded' ? servingBind(netConfig.data, WORKSPACE_ID !== null) : '';
  const tailnetReachable =
    host.kind === 'on-tailnet' &&
    tailnetAddressIsServed(bind, host.ip, window.location.hostname);

  const reload = useCallback(() => {
    // Fetched on EVERY platform: this is the only reading of concern 1 a
    // browser has, and it is also what proves a remote device is on the tailnet
    // when it reached us at a bare `100.x` address.
    setNetConfig({ status: 'loading' });
    getNetworkConfig().then(
      (data) => setNetConfig({ status: 'loaded', data }),
      (e) => setNetConfig(toFailed(e)),
    );
    // Also every platform: the MagicDNS name is the address the user copies to
    // another device, and a browser has no other way to learn it.
    setTailnetStatus({ status: 'loading' });
    getTailnetStatus().then(
      (data) => setTailnetStatus({ status: 'loaded', data }),
      (e) => setTailnetStatus(toFailed(e)),
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

  /** The Connect URLs section, plus its loading / failed states.
   *
   *  Rendered on EVERY platform. The tailnet rows come from two plain-HTTP
   *  reads a browser can make. So the address a user copies to their phone is
   *  on the page wherever they read it. Only the extra rows are machine-side:
   *  the localhost and LAN addresses need `lan_ip` and the Tauri bridge.
   *
   *  Every row is workspace-scoped. A bare gateway origin reaches the root,
   *  which redirects to the sole workspace or to the picker. That is the wrong
   *  address to hand out on a multi-workspace install.
   *
   *  All three loads share one failure branch, and the bridge only counts where
   *  there is one. A `netConfig` that failed cannot be rendered as an empty
   *  section: the bind it carries decides whether a plain-HTTP row is a live
   *  address or a dead one. */
  function connectUrlsSection() {
    // The shell and its TITLE render in every state, for the reason
    // `NetworkAccessPage` records for `access:network`: the anchor is a
    // navigation target, and `SettingsView` resolves it with ONE `querySelector`
    // on the mounting commit, then clears the target either way. An anchor that
    // waits for a fetch is missed on a cold open. Search Everywhere reaches this
    // one from a browser now, so it has to be there before the two reads land.
    const shell = (body: ComponentChildren) => (
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="access:urls">
        Connect URLs
        <Explainer title="Connect URLs">
          <p>
            The addresses that reach <strong>this workspace</strong>. Each carries the
            workspace's own path, so it lands here rather than wherever the gateway
            would send a bare address.
          </p>
          <p>Open one on another device to use Lucidos from your phone.</p>
        </Explainer>
        </div>
        {body}
      </div>
    );
    const failure =
      netConfig.status === 'failed' ? netConfig.error
      : tailnetStatus.status === 'failed' ? tailnetStatus.error
      : showMachineHalf && connectInfo.status === 'failed' ? connectInfo.error
      : null;
    if (failure !== null) {
      return shell(<LoadableError noun="connect info" error={failure} />);
    }
    // `connectUrlsReady` is the whole condition. The two status checks beside
    // it are what narrows the types below, which it cannot do from up there.
    if (!connectUrlsReady || netConfig.status !== 'loaded' || tailnetStatus.status !== 'loaded') {
      // Still delay-gated, so a fast load never flashes a loader. The shell
      // around it is not, per the anchor rule above.
      return shell(showLoading ? <div class="empty-state">Loading…</div> : null);
    }
    const tailnet = tailnetStatus.data;
    const connect = connectInfo.status === 'loaded' ? connectInfo.data : null;
    const tailnetRows = tailnetConnectRows({
      scope: SCOPE_PATH,
      here: window.location,
      tailnetIp: host.kind === 'on-tailnet' ? host.ip : null,
      magicDnsName: tailnet.magic_dns_name,
      workspaceServeUrl: tailnet.workspace_serve_url,
      bind: servingBind(netConfig.data, WORKSPACE_ID !== null),
    });
    // Nothing honest to list. Says so rather than rendering a bare heading, and
    // rather than vanishing: the anchor is a search destination, so an absent
    // section drops the reader at the top of the page with no explanation.
    if (connect === null && tailnetRows.length === 0) {
      return shell(
        <div class="settings-section-desc">
        No address reaches this workspace from another device yet. The Tailscale section
        below says what is missing.
        </div>,
      );
    }
    // `null`, not `{kind:'none'}`: off the bridge we cannot see this machine's
    // interfaces at all, and "No LAN address detected" would be a finding we
    // never made.
    const lan: LanRowState | null = connect
      ? lanRowAvoidingTailnet(
        netConfig.data.gateway_bind,
        connect.lan_ip,
        connect.tailscale.tailnet_ip,
        connect.port,
        )
      : null;
    return shell(
      <div class="list-rows">
        {connect && (
          <UrlRow
            label="This Mac"
            url={workspaceUrlAt(connect.localhost_url, SCOPE_PATH)}
            hint="localhost"
          />
        )}
        {lan?.kind === 'url' && (
          <UrlRow
            label="Local network"
            url={workspaceUrlAt(lan.url, SCOPE_PATH)}
            hint="same Wi-Fi · plain HTTP, so no app install or push (use Tailscale for those)"
          />
        )}
        {lan?.kind === 'disabled' && (
          <div class="list-row">
            <div class="list-row-info">
              <div class="title">Local network</div>
              <div class="list-row-details list-row-details-prose">
                Off: the gateway only listens on this Mac. Allow LAN access in Network access
                (applies after a restart).
              </div>
            </div>
            <div class="list-row-actions">
              {/* Network access is a section further down THIS page now (both
                  halves of "reach this engine from elsewhere" live under
                  Settings → Access), so this scrolls rather than switching
                  subview. `settingsScrollTarget` drives the scroll-and-mark
                  effect in SettingsView. */}
              <button class="action-btn" onClick={() => { settingsScrollTarget.value = 'access:network'; }}>
                Network access
              </button>
            </div>
          </div>
        )}
        {lan?.kind === 'none' && (
          <div class="list-row">
            <div class="list-row-info">
              <div class="title">Local network</div>
              <div class="list-row-details list-row-details-prose">No LAN address detected</div>
            </div>
          </div>
        )}
        {/* Derived by `tailnetConnectRows`, which owns the whole rule: the
            verified HTTPS URL when there is one, otherwise the plain-HTTP
            address, and only while something is listening on it. */}
        {tailnetRows.map((row) => (
          <UrlRow key={row.url} label={row.label} url={row.url} hint={row.hint} />
        ))}
      </div>,
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
                // Names the machine's HTTPS endpoint, NOT an address to open:
                // that one carries the workspace path and is in Connect URLs.
                ? <>Serving at <strong>{row.url}</strong> with an auto-renewed cert. The
                    address to open is under Connect URLs.</>
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
      {connectUrlsSection()}

      {/* The one static paragraph on this page, so the one thing behind an
          explainer. Everything else here is a `state.kind` branch of the setup
          walkthrough: a step the user is mid-way through following, which has to
          stay on screen while they act on it. The two numbered sections below
          already say that the machine and the device are separate steps. */}
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="access:tailscale">
          Tailscale (recommended)
          <Explainer title="Tailscale (recommended)">
            <p>
              Tailscale gives your other devices a private, encrypted HTTPS link to the
              machine running Lucidos that works off your home network, which is what
              enables the installable PWA and push notifications.
            </p>
            <p>
              It stays tailnet-private (we use <code>serve</code>, not{' '}
              <code>funnel</code>), so the engine is never exposed to the public
              internet.
            </p>
            <p>
              Two things have to be true, and they are independent: the machine is on a
              tailnet, and the device you want to use has joined it.
            </p>
          </Explainer>
        </div>
      </div>

      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="access:machine">
          1. The machine running Lucidos
        </div>
        {hostSection()}
      </div>

      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="access:device">
          2. This device
        </div>
        <DeviceTailscaleRow state={device} />
      </div>

      <DeviceStepsSection state={device} />
    </>
  );
}
