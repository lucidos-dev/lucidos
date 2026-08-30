/**
 * A phone browser installs before it pairs.
 *
 * iOS gives a home-screen app its own storage container, so a credential taken
 * in Safari never reaches it. Pairing the tab enrols the wrong device and
 * leaves the app the user actually opens still locked out. So the unpaired
 * screen sends a phone browser to install first, and the gateway has already
 * put the scanned code in the manifest's `start_url`.
 *
 * The screen itself uses hooks, so what it renders is pinned by a source scan,
 * the idiom `pairing-code-boxes.test.tsx` set. Everything decidable is a pure
 * function and is exercised directly.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import {
  installPlatformOf,
  installSteps,
  onPhoneCodeSource,
  pairingScreenBranch,
} from '../PairingGate';
import { clipboardAbilitiesOf } from '../../../utils/platform';

const here = dirname(fileURLToPath(import.meta.url));
const gateSrc: string = readFileSync(resolve(here, '../PairingGate.tsx'), 'utf8');

describe('which screen an unpaired client sees', () => {
  it('sends a phone BROWSER to install, and nothing else', () => {
    expect(pairingScreenBranch({ mobile: true, standalone: false })).toBe('install');
  });

  it('gives the form to the installed app, where pairing sticks', () => {
    expect(pairingScreenBranch({ mobile: true, standalone: true })).toBe('form');
  });

  it('leaves desktop exactly as it was', () => {
    // A desktop browser IS the device, and the host's own window self-pairs.
    // An install recipe on a Mac would be advice for a problem it cannot have.
    expect(pairingScreenBranch({ mobile: false, standalone: false })).toBe('form');
    expect(pairingScreenBranch({ mobile: false, standalone: true })).toBe('form');
  });
});

describe('the install steps', () => {
  it('routes on the platform in the user\'s hand', () => {
    expect(installPlatformOf({ ios: true, android: false })).toBe('ios');
    expect(installPlatformOf({ ios: false, android: true })).toBe('android');
    expect(installPlatformOf({ ios: false, android: false })).toBe('other');
  });

  it('names the iOS menu items rather than describing them', () => {
    const steps = installSteps('ios').join(' ');
    expect(steps).toContain('Share');
    expect(steps).toContain('Add to Home Screen');
  });

  it('ends every platform by opening the app that was installed', () => {
    // The last step is the one that pairs: the launch URL carries the code.
    for (const platform of ['ios', 'android', 'other'] as const) {
      const steps = installSteps(platform);
      expect(steps.length, platform).toBeGreaterThan(1);
      expect(steps[steps.length - 1], platform).toMatch(/^Open Lucidos/);
    }
  });
});

describe('the browser is advised, never locked out', () => {
  it('keeps an escape to the form', () => {
    // A borrowed phone, or somebody who wants no icon on their home screen.
    // Refusing outright would strand both.
    expect(gateSrc).toContain('Pair this browser instead');
    expect(
      /if \(branch === 'install' && !pairHere\) \{/.test(gateSrc),
      'the escape must be what reveals the form, not a second default',
    ).toBe(true);
  });

  it('tells an already-installed app how to get the code across', () => {
    // The manifest cannot reach an icon whose launch URL is already fixed, so
    // this case has to be named rather than left to be discovered.
    expect(gateSrc).toContain('Already have it on your home screen?');
    expect(gateSrc).toContain('Copy code');
    // Both routes into an existing install, since the clipboard can be empty by
    // the time the user gets there.
    expect(gateSrc).toContain('Paste code');
    expect(gateSrc).toContain('Scan QR');
  });
});

describe('a code that arrived in the launch URL', () => {
  it('is spent on sight, so the app really does pair itself', () => {
    // The manifest hands a fresh install its code in `start_url`. Leaving it
    // in a prefilled form makes the install screen's promise untrue, and hides
    // an expired code behind a tap on Pair.
    expect(gateSrc).toMatch(/useEffect\(\(\) => \{\s*if \(scanned\) \{\s*void redeem\(scanned\)/);
  });

  it('is spent WITHOUT painting a form that asks for it', () => {
    // The reported blink. The card said "Pair this device" for the length of
    // one round trip, about a device already pairing, and then reloaded.
    expect(gateSrc).toMatch(/useState<'running' \| 'done'>\(scanned \? 'running' : 'done'\)/);
    expect(
      /if \(!showAutoPair\) return null;/.test(gateSrc),
      'a fast redeem must draw nothing at all',
    ).toBe(true);
    // Only a refusal has a form to show. A success reloads under the splash.
    expect(gateSrc).toMatch(/if \(!paired\) setAutoPair\('done'\);/);
  });

  it('is the only code redeemed without a tap', () => {
    // A paste and a scan each already cost one, and the user is looking at the
    // field. Both fill it and stop.
    expect(gateSrc).toMatch(/setError\(null\);\s*setCode\(pasted\);/);
    expect(gateSrc).toMatch(/setCode\(scannedCode\);\s*setError\(null\);\s*setScanning\(false\);/);
  });
});

describe('what the pasteboard can do here', () => {
  it('reads copy and paste apart', () => {
    // `readText` is the one WebKit gates, so a surface offering Paste off the
    // back of a working Copy is a control that fails on tap.
    expect(clipboardAbilitiesOf({ writeText: async () => {}, readText: async () => '' })).toEqual({
      copy: true,
      paste: true,
    });
    expect(clipboardAbilitiesOf({ writeText: async () => {} })).toEqual({
      copy: true,
      paste: false,
    });
  });

  it('answers no to both when there is no clipboard at all', () => {
    // A plain-http LAN origin is not a secure context, and has neither.
    expect(clipboardAbilitiesOf(undefined)).toEqual({ copy: false, paste: false });
    expect(clipboardAbilitiesOf({})).toEqual({ copy: false, paste: false });
  });

  it('gates each control on its own ability', () => {
    expect(gateSrc).toMatch(/clipboardAbilities\(\)\.copy/);
    expect(gateSrc).toMatch(/clipboardAbilities\(\)\.paste/);
  });
});

describe('where an installed app is told to get a code', () => {
  it('names the phone browser, so recovery needs no second machine', () => {
    // The container that loses its credential is the installed one. The browser
    // on the same phone keeps its own, so it is a paired device and may mint a
    // code. This screen is the only place that could say so.
    const said = onPhoneCodeSource(true);
    expect(said).toBeTruthy();
    expect(said).toMatch(/phone browser/i);
    expect(said).toMatch(/Settings → Access/);
  });

  it('says nothing in a browser, which has no second container to reach', () => {
    expect(onPhoneCodeSource(false)).toBeNull();
  });

  it('shows it only when no launch code is being redeemed', () => {
    // A code already in hand makes the whole question moot, and the screen is
    // mid-redeem at that point.
    expect(gateSrc).toMatch(/\{!scanned && onPhoneSource &&/);
  });

  it('redeems only a launch code this client has not spent', () => {
    // An installed icon relaunches one `start_url` for good. An ungated
    // auto-redeem therefore fails on every cold launch, and spends the
    // gateway's wrong-guess budget doing it.
    expect(gateSrc).toMatch(/takeUnspentPairingCodeFromUrl\(\)/);
  });
});
