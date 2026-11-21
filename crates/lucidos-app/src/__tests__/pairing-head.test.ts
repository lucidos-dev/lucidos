import { describe, expect, it } from 'vitest';
import { SPENT_KEY } from '../utils/pairingCodeSeed';
// @ts-expect-error: Node is supplied by Vitest
import { readFileSync } from 'node:fs';

import { DEVICE_PAIRING_SECRET_PREFIX, PAIRING_CODE_DIGITS, PAIRING_SECRET_CHARS, PAIR_PARAM } from '../generated/gateway-constants';

const html: string = readFileSync(new URL('../../index.html', import.meta.url), 'utf8');
const scripts = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
const capture = scripts.find((s) => s.includes('var hasFragment'))!;
const manifest = scripts.find((s) => s.includes('var template'))!;

describe('pairing head bootstrap', () => {
  it('pins the inline grammar to the generated gateway contract', () => {
    expect(manifest).toContain(`[0-9]{${PAIRING_CODE_DIGITS}}`);
    expect(manifest).toContain(`${DEVICE_PAIRING_SECRET_PREFIX}[a-z2-7]{${PAIRING_SECRET_CHARS}}`);
    expect(capture).toContain(`fragment.has('${PAIR_PARAM}')`);
    expect(capture).toContain(`localStorage.getItem('${SPENT_KEY}')`);
  });

  it('lets an installed app resume normally after spending its fixed launch token', () => {
    const window: Record<string, unknown> = {};
    const location = new URL('https://example.test/~/#pair=01234567');
    const history = { state: null, replaceState: (_: unknown, _title: string, url: string) => { location.href = url; } };
    new Function('window', 'location', 'history', 'localStorage', capture)(window, location, history,
      { getItem: () => JSON.stringify(['01234567']) });
    expect(window.lucidosPairingLaunch).toBe(false);
    expect(window.lucidosPairingSeed).toBe('01234567');
    expect(location.hash).toBe('');
  });

  it.each([
    ['?pair=01234567&pick=#pair=87654321&thread=demo', '87654321', '?pick=#thread=demo'],
    ['?pair=01234567#thread=demo', '01234567', '#thread=demo'],
    ['?pair=bad&pair=other#pair=invalid&pair=duplicate', 'invalid', ''],
  ])('captures and strips before resources: %s', (input, seed, cleaned) => {
    const location = new URL(`https://example.test/~/${input}`);
    const window: Record<string, unknown> = {};
    const state = { previous: true };
    const history = { state, replaceState: (next: unknown, _: string, url: string) => {
      expect(next).toBe(state);
      location.href = url;
    } };
    new Function('window', 'location', 'history', capture)(window, location, history);
    expect(window.lucidosPairingSeed).toBe(seed);
    expect(window.lucidosPairingLaunch).toBe(true);
    expect(location.search + location.hash).toBe(cleaned);
    expect(html.indexOf(capture)).toBeLessThan(html.indexOf('<link'));
  });

  it('creates an install manifest with absolute references and a fragment launch URL', async () => {
    const link = { href: 'manifest.json' };
    const template = { textContent: JSON.stringify({ icons: [{ src: 'icons/icon-192.png' }] }) };
    const document = { getElementById: (id: string) => id === 'lucidos-pairing-manifest' ? template : link };
    let blob: Blob | undefined;
    class BootURL extends URL {
      static createObjectURL(value: Blob) { blob = value; return 'blob:local'; }
    }
    new Function('window', 'document', 'location', 'URL', 'Blob', manifest)(
      { lucidosPairingLaunch: true, lucidosPairingSeed: '01234567' }, document,
      new URL('https://example.test/~/'), BootURL, Blob,
    );
    expect(link.href).toBe('blob:local');
    expect(JSON.parse(await blob!.text())).toEqual({
      start_url: 'https://example.test/~/#pair=01234567',
      id: 'https://example.test/~/', scope: 'https://example.test/',
      icons: [{ src: 'https://example.test/~/icons/icon-192.png' }],
    });
  });
});
