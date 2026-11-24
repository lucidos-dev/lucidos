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
    for (const platform of ['ios', 'android', 'other'] as const) {
      const steps = installSteps(platform);
      expect(steps.length, platform).toBeGreaterThan(1);
      expect(steps[steps.length - 1], platform).toMatch(/^Open Lucidos/);
    }
  });
});

describe('the browser is advised, never locked out', () => {
  it('keeps an escape to the form', () => {
    expect(gateSrc).toContain('Pair this browser instead');
    expect(
      /if \(branch === 'install' && !pairHere\) \{/.test(gateSrc),
      'the escape must be what reveals the form, not a second default',
    ).toBe(true);
  });

  it('tells an already-installed app how to get the code across', () => {
    expect(gateSrc).toContain('Already have it on your home screen?');
    expect(gateSrc).toContain('Copy code');
    expect(gateSrc).toContain('Paste code');
    expect(gateSrc).toContain('Scan QR');
  });
});

describe('a code that arrived in the launch URL', () => {
  it('is spent on sight, so the app really does pair itself', () => {
    expect(gateSrc).toMatch(/useEffect\(\(\) => \{\s*if \(scanned\) \{\s*void redeem\(scanned\)/);
  });

  it('is spent WITHOUT painting a form that asks for it', () => {
    expect(gateSrc).toMatch(/useState<'running' \| 'done'>\(scanned \? 'running' : 'done'\)/);
    expect(
      /if \(!showAutoPair\) return null;/.test(gateSrc),
      'a fast redeem must draw nothing at all',
    ).toBe(true);
    expect(gateSrc).toMatch(/if \(!paired\) setAutoPair\('done'\);/);
  });

  it('is the only short code redeemed without a tap', () => {
    expect(gateSrc).toMatch(/setError\(null\);\s*adopt\(pasted\);/);
    expect(gateSrc).toMatch(/setScanning\(false\);\s*adopt\(scannedCode\);/);
    expect(gateSrc).toMatch(
      /if \(isPairingSecret\(token\)\) void redeem\(token\);\s*else setCode\(token\);/,
    );
  });
});

describe('what the pasteboard can do here', () => {
  it('reads copy and paste apart', () => {
    expect(clipboardAbilitiesOf({ writeText: async () => {}, readText: async () => '' })).toEqual({
      copy: true,
      paste: true,
      copyImage: false,
    });
    expect(clipboardAbilitiesOf({ writeText: async () => {} })).toEqual({
      copy: true,
      paste: false,
      copyImage: false,
    });
  });

  it('answers no to every one when there is no clipboard at all', () => {
    expect(clipboardAbilitiesOf(undefined)).toEqual({ copy: false, paste: false, copyImage: false });
    expect(clipboardAbilitiesOf({})).toEqual({ copy: false, paste: false, copyImage: false });
  });

  it('gates each control on its own ability', () => {
    expect(gateSrc).toMatch(/clipboardAbilities\(\)\.copy/);
    expect(gateSrc).toMatch(/clipboardAbilities\(\)\.paste/);
  });
});

describe('where an installed app is told to get a code', () => {
  it('names the phone browser, so recovery needs no second machine', () => {
    const said = onPhoneCodeSource(true);
    expect(said).toBeTruthy();
    expect(said).toMatch(/phone browser/i);
    expect(said).toMatch(/Settings → Access/);
  });

  it('says nothing in a browser, which has no second container to reach', () => {
    expect(onPhoneCodeSource(false)).toBeNull();
  });

  it('shows it only when no launch code is being redeemed', () => {
    expect(gateSrc).toMatch(/\{!scanned && onPhoneSource &&/);
  });

  it('redeems only a launch code this client has not spent', () => {
    expect(gateSrc).toMatch(/takeUnspentPairingCodeFromUrl\(\)/);
  });
});
