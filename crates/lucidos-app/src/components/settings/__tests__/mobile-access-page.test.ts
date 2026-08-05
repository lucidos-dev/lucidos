/**
 * The Mobile Access page must answer BOTH of its questions on every platform,
 * and gate only its native-only controls.
 *
 * Renamed from `mobile-access-row-reachable.test.ts` when the nav-level half of
 * this guard moved to `settings-nav-structure.test.ts`: no Settings category is
 * platform-gated at all now, which is a stronger rule than "this one row is
 * listed everywhere". What stays here is the PAGE's own behaviour, plus the one
 * assertion that `access` is still what renders it.
 *
 * The first bug this pins is the same shape as
 * `external-links-row-reachable.test.ts`, and is why that file's reasoning is
 * reused here. The nav entry was filtered to `isTauri() && enginePackaged.value`,
 * so the page existed only inside the packaged desktop app. Its whole purpose is
 * getting the user onto another device, and most of it needs no Tauri IPC at
 * all. Nothing failed: on a phone the row simply was not there.
 *
 * The second is what that fix left behind. The page still chose ONE of its two
 * concerns by platform, so a browser got the reading device's half alone, with
 * the machine's half suppressed. Both now render everywhere; only the ACTIONS
 * stay gated, because `get_connect_info`, `tailscale_up` and `tailscale_serve`
 * are Tauri commands with no engine HTTP equivalent, and a button rendered in a
 * browser throws the moment it is pressed. So this guard checks the gate and
 * the ungated reporting together: either one alone is a regression.
 *
 * Source-scan rather than a mounted render, for the reason the sibling file
 * gives: `SettingsView` pulls in the whole store, the model registry, OAuth and
 * device state.
 */
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

/** Strip comments so the prose explaining a rule can never stand in for it. */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\:])\/\/.*$/gm, '$1');
}

describe('Mobile Access reachability', () => {
  const nav = stripComments(SETTINGS_VIEW);
  const page = stripComments(PAGE);

  it('still lists the entry, rather than having quietly lost it', () => {
    // The page lives under the `access` category now, beside the network bind
    // it used to deep-link into. The nav-level half of this guard (no category
    // is platform-gated at all) moved to `settings-nav-structure.test.ts`; what
    // stays here is that this page is what `access` actually renders, because
    // deleting the case would satisfy that rule too.
    expect(nav).toContain(`case 'access': return accessSection();`);
    expect(nav).toContain('<MobileAccessPage />');
  });

  it('gates the native-only CONTROLS inside the page instead', () => {
    // Those controls are Tauri commands; the bridge dereferences
    // `window.__TAURI_INTERNALS__!` synchronously, so a button rendered in a
    // browser throws rather than rejecting.
    expect(page).toContain('const showMachineHalf = isTauri() && enginePackaged.value;');
    expect(page).toContain('{showMachineHalf && connectUrlsSection()}');
    // `tailscaleActionRow` is the only thing carrying Sign in / Expose, so it
    // must sit on the gated side of the branch and `HostTailnetRow` (reporting
    // only) on the other.
    expect(page).toContain('if (showMachineHalf) return tailscaleActionRow();');
  });

  it('reports a failed load on BOTH sides instead of checking forever', () => {
    // `hostTailnetState` folds failed into `unknown` because there is no honest
    // tailnet answer to give, so the render has to tell "could not ask" apart
    // from "have not asked yet". Rendering the neutral row for both is a
    // swallowed error: the section would sit on "Checking this machine".
    expect(page).toContain("netConfig.status === 'failed'");
    expect(page).toContain("connectInfo.status === 'failed'");
    expect(page.match(/<LoadableError noun="this machine's Tailscale state"/g) ?? []).toHaveLength(2);
  });

  it('skips only the BRIDGE fetch off the desktop app, never the HTTP one', () => {
    // `network-config` is plain HTTP and is the browser's only reading of the
    // machine's tailnet state, so the early return must come after it. Putting
    // the guard first is the regression: it leaves a browser with no way to know
    // the machine is on a tailnet, which is what made the page offer the install
    // to a working setup.
    const guard = page.indexOf('if (!showMachineHalf) return;');
    const httpFetch = page.indexOf('getNetworkConfig()');
    const bridgeFetch = page.indexOf('getConnectInfo()');
    expect(httpFetch).toBeGreaterThan(-1);
    expect(guard).toBeGreaterThan(httpFetch);
    expect(bridgeFetch).toBeGreaterThan(guard);
    // And a missing bridge is still not an error to report.
    expect(page).not.toContain('Mobile access is only available in the desktop app.');
  });

  it('renders BOTH concerns regardless of platform', () => {
    // The un-muddling. Neither section may be conditional on the platform gate:
    // what varies is how much each can say, never whether it appears.
    expect(page).toContain('1. The machine running Lucidos');
    expect(page).toContain('2. This device');
    expect(page).toContain('<DeviceTailscaleRow state={device} />');
    expect(page).toContain('<DeviceStepsSection state={device} />');
  });

  it('derives both concerns from live inputs, not from a constant', () => {
    // The device half used to be a constant, which told a phone reading over
    // its own tailnet to install Tailscale. `mobile-access-tailnet-state.test.ts`
    // pins both derivations; this pins that the page actually feeds them the
    // live inputs, including the host address that is the third proof.
    expect(page).toContain('deviceSetupState(');
    expect(page).toContain('window.location.hostname');
    expect(page).toContain('isStandalone()');
    expect(page).toContain('hostTailnetState(netConfig)');
    // Whether the app CAN be installed here is a separate fact from whether
    // Tailscale is connected, and a hardcoded `true` would put the page back to
    // naming an install control that plain HTTP never offers.
    expect(page).toContain('secureContext: window.isSecureContext');
  });

  it('never calls the reading device a phone', () => {
    // A desktop browser has no home screen, and this page is read from one as
    // often as from a handset. Wording that assumes a phone belongs behind
    // `isHandset()`, never in the unconditional copy.
    expect(page).toContain('function isHandset()');
    expect(page).not.toContain('onto this phone');
    expect(page).not.toContain('This phone is set up');
  });
});
