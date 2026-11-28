import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SETTINGS_VIEW = readFileSync(resolve(here, '..', 'SettingsView.tsx'), 'utf8');
const PAGE = readFileSync(resolve(here, '..', 'MobileAccessPage.tsx'), 'utf8');

function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\:])\/\/.*$/gm, '$1');
}

describe('Mobile Access reachability', () => {
  const nav = stripComments(SETTINGS_VIEW);
  const page = stripComments(PAGE);

  it('still lists the entry, rather than having quietly lost it', () => {
    expect(nav).toContain(`case 'access': return <GatewayAccessPage />;`);
  });

  it('gates the native-only CONTROLS inside the page instead', () => {
    expect(page).toContain('const showMachineHalf = isTauri() && enginePackaged.value;');
    expect(page).toContain('if (showMachineHalf) return tailscaleActionRow();');
  });

  it('renders Other addresses everywhere, because a URL is reporting', () => {
    expect(page).toContain('{otherAddressesSection()}');
    expect(page).not.toContain('{showMachineHalf && otherAddressesSection()}');
    expect(page).toContain('tailnetConnectRows({');
    expect(page).toContain('workspaceUrlAt(status.serve_origin, SCOPE_PATH)');
  });

  it('renders the Other addresses anchor in every state, not only when loaded', () => {
    expect(page.match(/data-search-anchor="access:addresses"/g) ?? []).toHaveLength(1);
    expect(page).toContain('const shell = (body: ComponentChildren) => (');
    expect(page).toContain(
      'return shell(<LoadableError noun="connect info" error={connectReadsError} />);',
    );
    expect(page).toContain('const settled = (body: ComponentChildren) => shell(');
    expect(page).toContain('return settled(null);');
  });

  it('scopes every printed URL to this workspace', () => {
    expect(page).toContain('workspaceUrlAt(lan, SCOPE_PATH)');
    expect(page).toContain('workspaceUrlAt(external, SCOPE_PATH)');
    expect(page).toContain('scope: SCOPE_PATH');
  });

  it('reports a failed load on BOTH sides instead of checking forever', () => {
    expect(page).toContain("netConfig.status === 'failed'");
    expect(page).toContain("connectInfo.status === 'failed'");
    expect(page.match(/<LoadableError noun="this machine's Tailscale state"/g) ?? []).toHaveLength(2);
  });

  it('skips only the BRIDGE fetch off the desktop app, never the HTTP one', () => {
    const guard = page.indexOf('if (!showMachineHalf) return');
    const httpFetch = page.indexOf('getGatewayNetworkConfig()');
    const tailnetFetch = page.indexOf('getGatewayTailnetStatus()');
    const bridgeFetch = page.indexOf('getConnectInfo()');
    expect(httpFetch).toBeGreaterThan(-1);
    expect(tailnetFetch).toBeGreaterThan(-1);
    expect(guard).toBeGreaterThan(httpFetch);
    expect(guard).toBeGreaterThan(tailnetFetch);
    expect(bridgeFetch).toBeGreaterThan(guard);
    expect(page).not.toContain('Mobile access is only available in the desktop app.');
  });

  it('renders BOTH concerns regardless of platform', () => {
    expect(page).toContain('1. The machine running Lucidos');
    expect(page).toContain('2. This device');
    expect(page).toContain('<DeviceTailscaleRow state={device} />');
    expect(page).toContain('<DeviceStepsSection state={device} />');
  });

  it('derives both concerns from live inputs, not from a constant', () => {
    expect(page).toContain('deviceSetupState(');
    expect(page).toContain('window.location.hostname');
    expect(page).toContain('isStandalone()');
    expect(page).toContain('hostTailnetState(netConfig, ');
    expect(page).toContain('secureContext: window.isSecureContext');
  });

  it('never calls the reading device a phone', () => {
    expect(page).toContain('function isHandset()');
    expect(page).not.toContain('onto this phone');
    expect(page).not.toContain('This phone is set up');
  });
});
