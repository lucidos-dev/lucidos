import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { qrImageSrc, expiryLabel, targetSentence } from './PairDeviceSection';
import { secondsLeft } from './useCodeCountdown';
import { findSettingsEntry, getSettingsSearchResults } from '../search/searchIndex';
import { vnodeToText } from '../chat/__tests__/vnodeToText';

describe('qrImageSrc', () => {
  it('wraps the SVG so an img can load it', () => {
    const src = qrImageSrc('<svg><rect/></svg>');
    expect(src.startsWith('data:image/svg+xml;charset=utf-8,')).toBe(true);
    expect(decodeURIComponent(src.split(',')[1])).toBe('<svg><rect/></svg>');
  });

  it('encodes the characters that would end the URL early', () => {
    const src = qrImageSrc('<svg fill="#000"/>');
    expect(src).not.toContain('#');
    expect(decodeURIComponent(src.split(',')[1])).toBe('<svg fill="#000"/>');
  });
});

describe('targetSentence', () => {
  it('names the address a QR will point at', () => {
    const text = vnodeToText(targetSentence({ kind: 'origin', url: 'https://mac.ts.net' }));
    expect(text).toContain('https://mac.ts.net');
  });

  it('says it is still working the address out, rather than claiming there is none', () => {
    const text = vnodeToText(targetSentence({ kind: 'resolving' }));
    expect(text).toMatch(/working out/i);
    expect(text).not.toMatch(/no address/i);
  });

  it('keeps "could not ask" apart from "there is none"', () => {
    const unknown = vnodeToText(targetSentence({ kind: 'unknown' }));
    const none = vnodeToText(targetSentence({ kind: 'none' }));
    expect(unknown).not.toBe(none);
    expect(unknown).toMatch(/could not/i);
    expect(unknown).not.toMatch(/no address here reaches/i);
    expect(none).toMatch(/no address here reaches/i);
  });
});

describe('secondsLeft', () => {
  it('counts whole seconds down to zero and stops', () => {
    expect(secondsLeft(10_000, 0)).toBe(10);
    expect(secondsLeft(10_000, 9_500)).toBe(1);
    expect(secondsLeft(10_000, 10_000)).toBe(0);
    expect(secondsLeft(10_000, 999_999)).toBe(0);
  });
});

describe('expiryLabel', () => {
  it('says something true at every point in the five minutes', () => {
    expect(expiryLabel(300)).toBe('Expires in 5:00');
    expect(expiryLabel(65)).toBe('Expires in 1:05');
    expect(expiryLabel(59)).toBe('Expires in 59s');
    expect(expiryLabel(1)).toBe('Expires in 1s');
  });

  it('calls an expired code expired, rather than counting to a small number', () => {
    expect(expiryLabel(0)).toBe('This code has expired');
    expect(expiryLabel(-5)).toBe('This code has expired');
  });

  it('carries the clock and nothing else', () => {
    expect(expiryLabel(300)).not.toMatch(/once/i);
  });
});

const here = dirname(fileURLToPath(import.meta.url));
const SECTION = readFileSync(resolve(here, 'PairDeviceSection.tsx'), 'utf8');
const PAGE = readFileSync(resolve(here, 'MobileAccessPage.tsx'), 'utf8');

function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\:])\/\/.*$/gm, '$1');
}

describe('Pair a device is reachable and honest', () => {
  const section = stripComments(SECTION);
  const page = stripComments(PAGE);

  it('renders its shell and anchor whatever the gateway can do', () => {
    const anchor = section.indexOf('data-search-anchor="access:pair-device"');
    const gate = section.indexOf('canMint ?');
    expect(anchor).toBeGreaterThan(-1);
    expect(gate).toBeGreaterThan(anchor);
  });

  it('is listed in the settings search index, under the words people use', () => {
    const entry = findSettingsEntry('access:pair-device');
    expect(entry?.subview).toBe('access');
    expect(entry?.anchor).toBe('access:pair-device');
    for (const term of ['pair', 'qr', 'pairing code']) {
      expect(
        getSettingsSearchResults(term, 20).some((r) => r.id === 'access:pair-device'),
        `"${term}" should reach this section`,
      ).toBe(true);
    }
  });

  it('is gated on the gateway being this page origin, as the switcher is', () => {
    expect(page).toContain('const gatewayIsOurOrigin = WORKSPACE_ID !== null;');
    expect(page).toContain('<PairDeviceSection canMint={gatewayIsOurOrigin}');
  });

  it('holds the mint button until the address is known', () => {
    expect(section).toContain("const resolving = target.kind === 'resolving';");
    expect(section).toContain('disabled={resolving || minted.status === \'loading\'}');
  });

  it('takes its origin from the shared derivation, never its own', () => {
    expect(page).toContain('pairingOrigin({');
    expect(section).not.toContain('magicDnsName');
    expect(section).not.toContain('location.hostname');
  });

  it('offers Copy only where there is a clipboard to copy to', () => {
    expect(section).toContain('clipboardAbilities().copy');
  });

  it('says "Or" on every card after the first, so the three read as alternatives', () => {
    expect(section).toContain('label="Or open this address"');
    expect(section).toContain("label={scanCard ? 'Or type this code' : 'Type this code'}");
  });

  it('renders no link at all, so the pairing address cannot become one', () => {
    expect(section).not.toContain('href=');
    expect(section).not.toContain('openExternalUrl');
  });

  it('mints only from a button press, so no code replaces itself', () => {
    const declaration = section.indexOf('const mint = useCallback');
    expect(declaration).toBeGreaterThan(-1);
    expect(section.match(/mintPairingCode\(/g)).toHaveLength(1);
    expect(section.indexOf('mintPairingCode(')).toBeGreaterThan(declaration);
    expect(section).not.toMatch(/\bmint\(/);
    expect(section).not.toMatch(/\(\s*mint\s*[,)]/);
  });
});
