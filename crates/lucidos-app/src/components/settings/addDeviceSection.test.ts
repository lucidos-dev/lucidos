/**
 * The Add a device section: its pure helpers, and the two things about the
 * section that a source scan is the only cheap way to pin.
 *
 * Mounting is not the tool here, for the reason the sibling page test gives:
 * `SettingsView` pulls in the whole store, the model registry, OAuth and
 * device state.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { qrImageSrc, secondsLeft, expiryLabel, targetSentence } from './AddDeviceSection';
import { findSettingsEntry, getSettingsSearchResults } from '../search/searchIndex';
import { vnodeToText } from '../chat/__tests__/vnodeToText';

describe('qrImageSrc', () => {
  it('wraps the SVG so an img can load it', () => {
    const src = qrImageSrc('<svg><rect/></svg>');
    expect(src.startsWith('data:image/svg+xml;charset=utf-8,')).toBe(true);
    expect(decodeURIComponent(src.split(',')[1])).toBe('<svg><rect/></svg>');
  });

  it('encodes the characters that would end the URL early', () => {
    // A raw `#` truncates a data URL at the fragment, and the QR then renders
    // as a broken image with no error anywhere.
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
    // The window this covers is short and real: the two reads take a moment,
    // and the section used to offer its button through it.
    const text = vnodeToText(targetSentence({ kind: 'resolving' }));
    expect(text).toMatch(/working out/i);
    expect(text).not.toMatch(/no address/i);
  });

  it('keeps "could not ask" apart from "there is none"', () => {
    // Conflating the two is a swallowed error: a failed read is not evidence
    // that no address exists. Same distinction `HostTailnetState` draws.
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
    // A machine that slept past the expiry must not report a negative wait.
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
    // A code left on screen stops working silently. The countdown exists so the
    // page stops claiming otherwise.
    expect(expiryLabel(0)).toBe('This code has expired');
    expect(expiryLabel(-5)).toBe('This code has expired');
  });

  it('carries the clock and nothing else', () => {
    // This is the one line on the section that is re-read every second, so it
    // says only what changes. Single use is a property nobody acts on.
    expect(expiryLabel(300)).not.toMatch(/once/i);
  });
});

const here = dirname(fileURLToPath(import.meta.url));
const SECTION = readFileSync(resolve(here, 'AddDeviceSection.tsx'), 'utf8');
const PAGE = readFileSync(resolve(here, 'MobileAccessPage.tsx'), 'utf8');

/** Strip comments so the prose explaining a rule can never stand in for it. */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\:])\/\/.*$/gm, '$1');
}

describe('Add a device is reachable and honest', () => {
  const section = stripComments(SECTION);
  const page = stripComments(PAGE);

  it('renders its shell and anchor whatever the gateway can do', () => {
    // The anchor is a Search Everywhere destination, resolved with one
    // `querySelector` on the mounting commit. A section that waits for a fetch,
    // or hides itself off the gateway, drops that hit at the top of the page.
    const anchor = section.indexOf('data-search-anchor="access:add-device"');
    const gate = section.indexOf('canMint ?');
    expect(anchor).toBeGreaterThan(-1);
    expect(gate).toBeGreaterThan(anchor);
  });

  it('is listed in the settings search index, under the words people use', () => {
    const entry = findSettingsEntry('access:add-device');
    expect(entry?.subview).toBe('access');
    expect(entry?.anchor).toBe('access:add-device');
    for (const term of ['pair', 'qr', 'pairing code']) {
      expect(
        getSettingsSearchResults(term, 20).some((r) => r.id === 'access:add-device'),
        `"${term}" should reach this section`,
      ).toBe(true);
    }
  });

  it('is gated on the gateway being this page origin, as the switcher is', () => {
    // `/~/…` is an absolute path. Off the gateway it resolves to the engine,
    // which 404s every one of these routes.
    expect(page).toContain('const gatewayIsOurOrigin = WORKSPACE_ID !== null;');
    expect(page).toContain('<AddDeviceSection canMint={gatewayIsOurOrigin}');
  });

  it('holds the mint button until the address is known', () => {
    // Otherwise a click inside the read window mints a code with no QR, on a
    // machine that had an address for one.
    expect(section).toContain("const resolving = target.kind === 'resolving';");
    expect(section).toContain('disabled={resolving || minted.status === \'loading\'}');
  });

  it('takes its origin from the shared derivation, never its own', () => {
    // The QR and the Connect URLs Tailscale row must not name different hosts.
    expect(page).toContain('pairingOrigin({');
    expect(section).not.toContain('magicDnsName');
    expect(section).not.toContain('location.hostname');
  });

  it('offers Copy only where there is a clipboard to copy to', () => {
    // A non-secure context exposes no `navigator.clipboard`, and this page is
    // read over a plain-HTTP LAN address often enough for that to matter. Same
    // gate the pairing screen puts on its own Copy and Paste.
    expect(section).toContain('clipboardAbilities().copy');
  });

  it('says "Or" on every card after the first, so the three read as alternatives', () => {
    // Three bare imperatives side by side read as a checklist of three things
    // to do. The conditional is the other half: with no QR the code card IS
    // the first, and an "Or" there would answer nothing.
    expect(section).toContain('label="Or open this address"');
    expect(section).toContain("label={scanCard ? 'Or type this code' : 'Type this code'}");
  });

  it('renders no link at all, so the pairing address cannot become one', () => {
    // The address is for the OTHER device. Following it here would spend a
    // single-use code on a device that is already paired. Asserted on the
    // DESTINATION rather than on the anchor tag: an `onClick` opener and an
    // `href` written after destructuring are the two ways back in, and a
    // tag-shaped pattern catches neither.
    expect(section).not.toContain('href=');
    expect(section).not.toContain('openExternalUrl');
  });

  it('mints only from a button press, so no code replaces itself', () => {
    // The section used to renew an expired code by itself, which ADR 0098
    // reversed. Three ways back in, and each is shut here: a second call to
    // the API, a call to the callback, and the callback handed to a timer.
    // What is left is a button passing it as `onClick={mint}`.
    const declaration = section.indexOf('const mint = useCallback');
    expect(declaration).toBeGreaterThan(-1);
    expect(section.match(/mintPairingCode\(/g)).toHaveLength(1);
    expect(section.indexOf('mintPairingCode(')).toBeGreaterThan(declaration);
    expect(section).not.toMatch(/\bmint\(/);
    expect(section).not.toMatch(/\(\s*mint\s*[,)]/);
  });
});
