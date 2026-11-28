import type { ComponentChildren } from 'preact';
import { useState, useEffect, useLayoutEffect, useCallback } from 'preact/hooks';
import { showToast, enginePackaged, tailscaleServeRun, settingsScrollTarget } from '../../store/store';
import {
  beginTailscaleServeRun,
  clearTailscaleServeRun,
  applyTailscaleServeProgress,
} from '../../store/actions/backgroundActivity';
import { isTauri, isIOS, isAndroid, isStandalone } from '../../utils/platform';
import { openExternalUrl } from '../../utils/openExternalUrl';
import { copyToClipboard } from '../../utils/clipboard';
import {
  getConnectInfo,
  tailscaleUp,
  tailscaleServe,
  openExternal,
  type ConnectInfo,
  type TailscaleInfo,
} from '../../utils/tauri';
import {
  getExternalAddress,
  getGatewayNetworkConfig,
  getGatewayTailnetStatus,
  getGatewayTls,
  type ExternalAddressInfo,
  type GatewayNetworkConfig,
  type GatewayTailnetStatus,
  type GatewayTls,
} from '../../api/client/control';
import { GatewayHttpsRow } from './GatewayHttpsRow';
import { SCOPE_PATH, WORKSPACE_ID } from '../../utils/basePath';
import { Explainer } from '../shared/Explainer';
import { Disclosure } from '../shared/Disclosure';
import { DisclosureChevron } from '../shared/DisclosureChevron';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText } from '../shared/Skeleton';
import { PairGatewaySection, RemoteGatewaysSection } from './GatewayPairingSections';
import { PairDeviceSection } from './PairDeviceSection';
import { PairingRequestsSection } from './PairingRequestsSection';
import { ExternalAddressSection } from './ExternalAddressSection';
import type { PairTarget } from './PairDeviceSection';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { loadingIfFresh, toFailed } from '../../store/types';
import type { Loadable } from '../../store/types';
import { errorDetail } from '../../utils/errorDetail';
import { LoadableError } from '../shared/LoadableError';
import { isLoopbackHostname } from '../../utils/loopbackHostname';

const gatewayIsOurOrigin = WORKSPACE_ID !== null;

const TAILSCALE_DOWNLOAD_URL = 'https://tailscale.com/download';
const TAILSCALE_IOS_URL = 'https://apps.apple.com/app/tailscale/id1470499037';
const TAILSCALE_ANDROID_URL = 'https://play.google.com/store/apps/details?id=com.tailscale.ipn';

export function tailscaleDownloadUrl(platform: { ios: boolean; android: boolean }): string {
  if (platform.ios) return TAILSCALE_IOS_URL;
  if (platform.android) return TAILSCALE_ANDROID_URL;
  return TAILSCALE_DOWNLOAD_URL;
}

export function openTailscaleDownload(): void {
  const url = tailscaleDownloadUrl({ ios: isIOS(), android: isAndroid() });
  if (!isTauri()) {
    openExternalUrl(url);
    return;
  }
  openExternal(url).catch((e) => {
    showToast(`Couldn't open ${url}: ${errorDetail(e)}`, 'error');
  });
}

export function lanUrl(gatewayBind: string, lanIp: string | null, port: number): string | null {
  const bind = gatewayBind.trim();
  if (bind === '' || bind === 'loopback') return null;
  if (bind === 'all') return lanIp ? `http://${lanIp}:${port}` : null;
  return `http://${bind}:${port}`;
}

export function tailnetHttpUrl(
  gatewayBind: string,
  tailnetIp: string | null,
  port: number,
): string | null {
  if (!tailnetIp) return null;
  return tailnetIsServed(gatewayBind, tailnetIp) ? `http://${tailnetIp}:${port}` : null;
}

export function tailnetIsServed(gatewayBind: string, tailnetIp: string): boolean {
  const bind = gatewayBind.trim();
  return bind === 'all' || bind === tailnetIp;
}

export function lanUrlAvoidingTailnet(
  gatewayBind: string,
  lanIp: string | null,
  tailnetIp: string | null,
  port: number,
): string | null {
  const lan = lanUrl(gatewayBind, lanIp, port);
  return lan !== null && lan === tailnetHttpUrl(gatewayBind, tailnetIp, port) ? null : lan;
}

export function workspaceUrlAt(origin: string, scope: string): string {
  const base = origin.replace(/\/+$/, '');
  return `${base}${scope.startsWith('/') ? scope : `/${scope}`}`;
}

export function originAtHost(here: { protocol: string; port: string }, host: string): string {
  return `${here.protocol}//${host}${here.port ? `:${here.port}` : ''}`;
}

export type ConnectUrlRow = { label: string; url: string; hint: string };

export function tailnetConnectRows(input: TailnetInput & { scope: string }): ConnectUrlRow[] {
  if (input.gatewayHttps) {
    return [{ label: 'Tailscale', url: workspaceUrlAt(input.gatewayHttps, input.scope), hint: tailnetHint(true) }];
  }
  if (input.workspaceServeUrl) {
    return [{ label: 'Tailscale', url: input.workspaceServeUrl, hint: tailnetHint(true) }];
  }
  const origin = tailnetOrigin(input);
  if (origin === null) return [];
  return [{ label: 'Tailscale', url: workspaceUrlAt(origin, input.scope), hint: tailnetHint(false) }];
}

export interface TailnetInput {
  here: { protocol: string; hostname: string; port: string };
  tailnetIp: string | null;
  magicDnsName: string | null;
  workspaceServeUrl: string | null;
  gatewayHttps: string | null;
  bind: string;
}

export function tailnetOrigin(input: TailnetInput): string | null {
  if (input.gatewayHttps) return input.gatewayHttps;
  if (input.workspaceServeUrl) return originOf(input.workspaceServeUrl);
  if (input.here.protocol !== 'http:') return null;
  const ip = input.tailnetIp;
  if (!ip || !tailnetServesThisReader(input.bind, ip, input.here.hostname)) return null;
  return originAtHost(input.here, input.magicDnsName ?? ip);
}

function originOf(url: string): string | null {
  try {
    return new URL(url).origin;
  } catch {
    return null;
  }
}

export function pairingOrigin(
  input: TailnetInput & { external: string | null; lan: string | null },
): string | null {
  const reader =
    input.here.protocol !== 'http:' || isLoopbackHostname(input.here.hostname)
      ? null
      : originAtHost(input.here, input.here.hostname);
  return (
    input.external ??
    tailnetOrigin(input) ??
    reader ??
    input.lan
  );
}

function tailnetHint(secure: boolean): string {
  return secure
    ? 'anywhere on your tailnet · HTTPS, so the app install and push work here'
    : 'anywhere on your tailnet · plain HTTP, so no app install or push yet';
}

export function tailnetAddressIsServed(
  bind: string,
  tailnetIp: string,
  readerHostname: string,
): boolean {
  return readerHostname.trim() === tailnetIp.trim() || tailnetIsServed(bind, tailnetIp);
}

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

export type TailscaleRowState =
  | { kind: 'get' }
  | { kind: 'sign-in'; canRun: boolean; reason: string | null }
  | { kind: 'expose'; canRun: boolean; magicDnsName: string | null }
  | { kind: 'serving'; url: string; canRun: boolean };

export function tailscaleRowState(ts: TailscaleInfo): TailscaleRowState {
  if (!ts.on_tailnet) {
    return ts.installed
      ? { kind: 'sign-in', canRun: ts.cli_available, reason: ts.reason }
      : { kind: 'get' };
  }
  if (ts.serve_url) return { kind: 'serving', url: ts.serve_url, canRun: ts.cli_available };
  return { kind: 'expose', canRun: ts.cli_available, magicDnsName: ts.magic_dns_name };
}

export type HostTailnetState =
  | { kind: 'on-tailnet'; ip: string }
  | { kind: 'no-tailnet'; reason: string | null }
  | { kind: 'unknown' };

export function hostTailnetState(
  config: Loadable<{ detected_tailscale_ip: string | null }>,
  reason: string | null,
): HostTailnetState {
  if (config.status !== 'loaded') return { kind: 'unknown' };
  const ip = config.data.detected_tailscale_ip;
  return ip ? { kind: 'on-tailnet', ip } : { kind: 'no-tailnet', reason };
}

export function isTailnetHostname(hostname: string): boolean {
  return hostname.trim().toLowerCase().endsWith('.ts.net');
}

export type DeviceSetupState =
  | { kind: 'same-machine' }
  | { kind: 'join-tailnet' }
  | { kind: 'needs-https'; hostname: string }
  | { kind: 'install-app'; hostname: string }
  | { kind: 'ready'; hostname: string };

export function deviceSetupState(device: {
  hostname: string;
  standalone: boolean;
  secureContext: boolean;
  hostTailnetIp: string | null;
  hostMagicDnsName: string | null;
}): DeviceSetupState {
  const hostname = device.hostname.trim();
  if (isLoopbackHostname(hostname)) return { kind: 'same-machine' };
  const isHost = (name: string | null) =>
    name !== null && hostname.toLowerCase() === name.trim().toLowerCase();
  const onTailnet =
    isTailnetHostname(hostname) || isHost(device.hostTailnetIp) || isHost(device.hostMagicDnsName);
  if (!onTailnet) return { kind: 'join-tailnet' };
  if (device.standalone) return { kind: 'ready', hostname };
  return device.secureContext
    ? { kind: 'install-app', hostname }
    : { kind: 'needs-https', hostname };
}

function UrlRow({ label, url, hint }: { label?: string; url?: string; hint?: string }) {
  const copy = useCallback(() => { if (url) copyToClipboard(url); }, [url]);
  return (
    <div class="list-row">
      <div class="list-row-info">
        <SkText class="title" as="div" w="6rem">{label}</SkText>
        <SkText class="list-row-details list-row-details-prose" as="div" w="18rem">
          {url && (
            <>
              <button class="mobile-access-url-button accent-link" onClick={copy}>{url}</button>
              {hint && <> &middot; {hint}</>}
            </>
          )}
        </SkText>
      </div>
      <div class="list-row-actions">
        <SkBlock w="3.5rem" h="2rem" round>
          <button class="action-btn" onClick={copy}>Copy</button>
        </SkBlock>
      </div>
    </div>
  );
}

function isHandset(): boolean {
  return isIOS() || isAndroid();
}

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
        : 'It is NOT listening on that address though, so nothing off this machine can connect to it there yet: allow it in the workspace picker\'s Network access control, which takes effect after a restart.'}{' '}
        Adding HTTPS with <code>tailscale serve</code> is what enables the installable app and
        push. That needs the Tailscale command-line tool, and it works whatever the address above
        says, since it proxies from this machine to <code>127.0.0.1</code>.
      </InfoRow>
    );
  }
  if (readerIsHost) return <InstallTailscaleRow onHost={true} />;
  return (
    <InfoRow title="This machine is not on a tailnet">
      {state.reason ? <>{state.reason}{' '}</> : null}
      The machine running Lucidos has no tailnet address, so no device can reach it over Tailscale.
      Install Tailscale there and sign it in to your tailnet.
    </InfoRow>
  );
}

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

function DeviceStepsSection({ state }: { state: DeviceSetupState }) {
  const handset = isHandset();
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

export function MobileAccessPage() {
  if (gatewayIsOurOrigin) return <GatewayAccessPage />;
  return (
    <div class="settings-section">
      <div class="settings-section-desc">
        No gateway serves this workspace, so there is no pairing, address or Tailscale state to
        show. Open it through the Lucidos gateway to manage access.
      </div>
    </div>
  );
}

function GatewayAccessPage() {
  const showMachineHalf = isTauri() && enginePackaged.value;
  const [connectInfo, setConnectInfo] = useState<Loadable<ConnectInfo>>({ status: 'not-loaded' });
  const [netConfig, setNetConfig] = useState<Loadable<GatewayNetworkConfig>>({ status: 'not-loaded' });
  const [tailnetStatus, setTailnetStatus] = useState<Loadable<GatewayTailnetStatus>>({ status: 'not-loaded' });
  const [externalAddress, setExternalAddressState] = useState<Loadable<ExternalAddressInfo>>({ status: 'not-loaded' });
  const [gatewayTls, setGatewayTls] = useState<Loadable<GatewayTls>>({ status: 'not-loaded' });
  const [busy, setBusy] = useState<null | 'up'>(null);
  const serveRunning = tailscaleServeRun.value !== null;
  const [authKey, setAuthKey] = useState('');
  const addressesHit = settingsScrollTarget.value === 'access:addresses';
  const [addressesOpen, setAddressesOpen] = useState(addressesHit);
  const addressesReady =
    netConfig.status === 'loaded' &&
    tailnetStatus.status === 'loaded' &&
    (!showMachineHalf || connectInfo.status === 'loaded') &&
    (gatewayTls.status === 'loaded' || gatewayTls.status === 'failed');
  const showLoading = useDelayedFlag(!addressesReady);

  const tailnet = tailnetStatus.status === 'loaded' ? tailnetStatus.data : null;
  const host: HostTailnetState = showMachineHalf && connectInfo.status === 'loaded'
    ? (connectInfo.data.tailscale.tailnet_ip
        ? { kind: 'on-tailnet', ip: connectInfo.data.tailscale.tailnet_ip }
        : { kind: 'no-tailnet', reason: connectInfo.data.tailscale.reason })
    : hostTailnetState(netConfig, tailnet?.reason ?? null);
  const device: DeviceSetupState = deviceSetupState({
    hostname: window.location.hostname,
    standalone: isStandalone(),
    secureContext: window.isSecureContext,
    hostTailnetIp: host.kind === 'on-tailnet' ? host.ip : null,
    hostMagicDnsName: tailnet?.magic_dns_name ?? null,
  });
  const bind = netConfig.status === 'loaded' ? netConfig.data.gateway_bind : '';
  const tailnetReachable =
    host.kind === 'on-tailnet' &&
    tailnetAddressIsServed(bind, host.ip, window.location.hostname);

  const lan: string | null =
    connectInfo.status === 'loaded' && netConfig.status === 'loaded'
      ? lanUrlAvoidingTailnet(
        netConfig.data.gateway_bind,
        connectInfo.data.lan_ip,
        connectInfo.data.tailscale.tailnet_ip,
        connectInfo.data.port,
        )
      : null;
  const connectReadsError =
    netConfig.status === 'failed' ? netConfig.error
    : tailnetStatus.status === 'failed' ? tailnetStatus.error
    : showMachineHalf && connectInfo.status === 'failed' ? connectInfo.error
    : null;
  const addressesFailed = connectReadsError !== null;
  useLayoutEffect(() => {
    if (addressesHit || addressesFailed) setAddressesOpen(true);
  }, [addressesHit, addressesFailed]);
  const external = externalAddress.status === 'loaded' ? externalAddress.data.external_address : null;
  const tailnetInput = (status: GatewayTailnetStatus): TailnetInput => ({
    here: window.location,
    tailnetIp: host.kind === 'on-tailnet' ? host.ip : null,
    magicDnsName: status.magic_dns_name,
    workspaceServeUrl: status.serve_origin === null ? null : workspaceUrlAt(status.serve_origin, SCOPE_PATH),
    gatewayHttps: gatewayTls.status === 'loaded' ? gatewayTls.data.https_origin : null,
    bind,
  });
  const tailnetDefault = tailnetStatus.status === 'loaded'
    ? tailnetOrigin(tailnetInput(tailnetStatus.data))
    : null;

  function pairTarget(): PairTarget {
    if (connectReadsError !== null || externalAddress.status === 'failed' || gatewayTls.status === 'failed') {
      return { kind: 'unknown' };
    }
    if (
      !addressesReady ||
      tailnetStatus.status !== 'loaded' ||
      externalAddress.status !== 'loaded' ||
      gatewayTls.status !== 'loaded'
    ) {
      return { kind: 'resolving' };
    }
    const url = pairingOrigin({ ...tailnetInput(tailnetStatus.data), external, lan });
    return url === null ? { kind: 'none' } : { kind: 'origin', url };
  }

  const reload = useCallback((): Promise<unknown> => {
    setNetConfig(loadingIfFresh);
    const reads: Promise<void>[] = [getGatewayNetworkConfig().then(
      (data) => setNetConfig({ status: 'loaded', data }),
      (e) => setNetConfig(toFailed(e)),
    )];
    setTailnetStatus(loadingIfFresh);
    reads.push(getGatewayTailnetStatus().then(
      (data) => setTailnetStatus({ status: 'loaded', data }),
      (e) => setTailnetStatus(toFailed(e)),
    ));
    setExternalAddressState(loadingIfFresh);
    reads.push(getExternalAddress().then(
      (data) => setExternalAddressState({ status: 'loaded', data }),
      (e) => setExternalAddressState(toFailed(e)),
    ));
    setGatewayTls(loadingIfFresh);
    reads.push(getGatewayTls().then(
      (data) => setGatewayTls({ status: 'loaded', data }),
      (e) => setGatewayTls(toFailed(e)),
    ));
    if (!showMachineHalf) return Promise.all(reads);
    setConnectInfo(loadingIfFresh);
    reads.push(getConnectInfo().then(
      (data) => setConnectInfo({ status: 'loaded', data }),
      (e) => setConnectInfo(toFailed(e)),
    ));
    return Promise.all(reads);
  }, [showMachineHalf]);

  useEffect(() => { void reload(); }, [reload]);
  usePanelRefresh('mobile access', reload);

  const onUp = useCallback(async () => {
    setBusy('up');
    try {
      await tailscaleUp(authKey.trim() || undefined);
      void reload();
    } catch (e) {
      showToast(errorDetail(e), 'error');
    } finally {
      setAuthKey('');
      setBusy(null);
    }
  }, [authKey, reload]);

  const onServe = useCallback(async () => {
    if (tailscaleServeRun.value) return;
    beginTailscaleServeRun();
    try {
      const url = await tailscaleServe();
      if (tailscaleServeRun.value) {
        applyTailscaleServeProgress({ phase: 'done', url });
      }
      void reload();
    } catch (e) {
      if (tailscaleServeRun.value) {
        clearTailscaleServeRun();
        showToast(errorDetail(e), 'error');
      }
    }
  }, [reload]);

  function otherAddressesSection() {
    const shell = (body: ComponentChildren) => (
      <div class="settings-section">
        <button
          type="button"
          class="settings-disclosure-toggle"
          data-search-anchor="access:addresses"
          aria-expanded={addressesOpen}
          onClick={() => setAddressesOpen(!addressesOpen)}
        >
          <DisclosureChevron open={addressesOpen} />
          Other addresses
        </button>
        <Disclosure open={addressesOpen}>{body}</Disclosure>
      </div>
    );
    if (connectReadsError !== null) {
      return shell(<LoadableError noun="connect info" error={connectReadsError} />);
    }
    const settled = (body: ComponentChildren) => shell(
      <LoadingFade
        showSkeleton={showLoading}
        skeleton={<ListSkeletonOf count={2} containerClass="list-rows" row={() => <UrlRow />} />}
      >
        {body}
      </LoadingFade>,
    );
    if (!addressesReady || netConfig.status !== 'loaded' || tailnetStatus.status !== 'loaded') {
      return settled(null);
    }
    const tailnetRows = tailnetConnectRows({ ...tailnetInput(tailnetStatus.data), scope: SCOPE_PATH });
    if (lan === null && tailnetRows.length === 0 && external === null) {
      return settled(
        <div class="settings-section-desc">
        No address reaches this workspace from another device yet. The Tailscale section
        below says what is missing.
        </div>,
      );
    }
    return settled(
      <div class="list-rows">
        {lan !== null && (
          <UrlRow
            label="Local network"
            url={workspaceUrlAt(lan, SCOPE_PATH)}
            hint="same Wi-Fi · plain HTTP, so no app install or push (use Tailscale for those)"
          />
        )}
        {tailnetRows.map((row) => (
          <UrlRow key={row.url} label={row.label} url={row.url} hint={row.hint} />
        ))}
        {external !== null && (
          <UrlRow
            label="External address"
            url={workspaceUrlAt(external, SCOPE_PATH)}
            hint={external.startsWith('https:') ? 'your own route · HTTPS' : 'your own route · plain HTTP, so no app install or push'}
          />
        )}
      </div>,
    );
  }

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

  function tailscaleActionRow() {
    if (connectInfo.status === 'failed') {
      return <LoadableError noun="this machine's Tailscale state" error={connectInfo.error} />;
    }
    if (connectInfo.status !== 'loaded') {
      return <HostTailnetRow state={{ kind: 'unknown' }} readerIsHost={true} reachable={false} />;
    }
    const row = tailscaleRowState(connectInfo.data.tailscale);

    if (row.kind === 'get') return <InstallTailscaleRow onHost={true} />;

    if (row.kind === 'sign-in') {
      return (
        <div class="list-rows">
          <div class="list-row repo-add-form">
            <div class="list-row-info" style={{ gap: '0.5rem' }}>
              <div class="title">Sign in to Tailscale</div>
              <div class="list-row-details list-row-details-prose">
                {row.reason ? <>{row.reason}{' '}</> : null}
                {row.canRun
                  ? 'Opens a browser to join your tailnet. Optionally paste a pre-authorized auth key.'
                  : 'Open the Tailscale app in your menu bar and sign in there. This Mac has no Tailscale command-line tool, which is what we would need to do it for you.'}
              </div>
              {row.canRun && (
                <input
                  class="device-name-input"
                  type="password"
                  placeholder="Auth key (optional): tskey-auth-…"
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
                ? <>Serving at <strong>{row.url}</strong> with an auto-renewed cert. The
                    address to open is under Other addresses.</>
                : row.canRun
                  ? 'Sets up tailnet HTTPS for the engine, which is what enables the installable PWA and push. Setup runs in the background and reports on the Lucidos badge, and it may ask you to enable Serve for your tailnet.'
                  : <>Your phone can already reach the plain-HTTP address above. HTTPS is what adds the installable PWA and push, and it needs <code>tailscale serve</code>. Install the Tailscale command-line tool to set it up: use Install CLI in the Tailscale app, or <code>brew install tailscale</code>.</>}
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
      <PairingRequestsSection available={gatewayIsOurOrigin} />

      <PairDeviceSection canMint={gatewayIsOurOrigin} target={pairTarget()} />

      {otherAddressesSection()}

      <PairGatewaySection available={gatewayIsOurOrigin} target={pairTarget()} />
      <RemoteGatewaysSection available={gatewayIsOurOrigin} />

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
        <GatewayHttpsRow tls={gatewayTls} />
      </div>

      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="access:device">
          2. This device
        </div>
        <DeviceTailscaleRow state={device} />
      </div>

      <DeviceStepsSection state={device} />

      <ExternalAddressSection
        available={gatewayIsOurOrigin}
        info={externalAddress}
        tailnetDefault={tailnetDefault}
        onSaved={(data) => setExternalAddressState({ status: 'loaded', data })}
      />
    </>
  );
}
