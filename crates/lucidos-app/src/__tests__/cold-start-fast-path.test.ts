/**
 * The picker's cold-start fast path, run for real.
 *
 * It is an inline `<head>` script in `index.html`, so nothing imports it and a
 * source scan is the usual fallback. Here the script is lifted out and called
 * with stub globals instead, which is worth the extraction: every rule in it is
 * a stand-down, and a scan can only prove the words are present.
 *
 * The `?pair=` stand-down is why this suite exists. A scanned QR lands on
 * `/~/?pair=<code>`, a public path where the gateway stamps the code into the
 * manifest `start_url` an Add-to-Home-Screen install launches with. This script
 * preserves the query through its redirect, so a phone that remembers a
 * workspace carried the code onto `/<slug>/`, which is gated. The unpaired
 * caller got the pairing shell there, its manifest carried no code, and the
 * installed app opened asking to be paired.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const INDEX_HTML: string = readFileSync(resolve(here, '../../index.html'), 'utf8');

/** The body of the inline script that redirects to the remembered workspace.
 *  Anchored on the `location.replace` call, since two scripts read the key and
 *  only this one acts on it. */
function fastPathSource(): string {
  const key = "location.replace('/' + encodeURIComponent(slug)";
  const at = INDEX_HTML.indexOf(key);
  expect(at, 'no inline script redirects to the remembered workspace').toBeGreaterThanOrEqual(0);
  const open = INDEX_HTML.lastIndexOf('<script>', at);
  const close = INDEX_HTML.indexOf('</script>', at);
  expect(open, 'unterminated script').toBeGreaterThanOrEqual(0);
  return INDEX_HTML.slice(open + '<script>'.length, close);
}

interface Env {
  /** The `<base href>` the gateway stamped, or null for no base element. */
  base?: string | null;
  /** Is the document marked as the pairing shell? */
  pairingMeta?: boolean;
  /** The remembered workspace slug, or null. */
  slug?: string | null;
  search?: string;
  hash?: string;
}

/** Run the fast path against `env` and answer the URL it replaced with, if any. */
function run(env: Env): string | null {
  const { base = '/~/', pairingMeta = false, slug = 'demo', search = '', hash = '' } = env;
  let replaced: string | null = null;
  const document = {
    querySelector(selector: string) {
      if (selector === 'base') return base === null ? null : { getAttribute: () => base };
      if (selector === 'meta[name="lucidos-pairing"]') return pairingMeta ? {} : null;
      return null;
    },
  };
  const localStorage = { getItem: () => slug };
  const location = { search, hash, replace: (url: string) => void (replaced = url) };
  // eslint-disable-next-line no-new-func
  new Function('document', 'localStorage', 'location', fastPathSource())(
    document,
    localStorage,
    location,
  );
  return replaced;
}

describe('picker cold-start fast path', () => {
  it('opens the remembered workspace, carrying the query and hash', () => {
    expect(run({})).toBe('/demo/');
    expect(run({ search: '?notification=n1', hash: '#thread=t1' })).toBe(
      '/demo/?notification=n1#thread=t1',
    );
  });

  it('stands down for a scanned pairing code', () => {
    // The whole point: `/~/` is public and stamps the code into the manifest,
    // `/<slug>/` is gated and cannot.
    expect(run({ search: '?pair=01234567' })).toBeNull();
    // Whatever the value looks like. The gateway owns the grammar, and a
    // redirect helps no pairing navigation.
    expect(run({ search: '?pair=' })).toBeNull();
    expect(run({ search: '?pair=abc&notification=n1' })).toBeNull();
  });

  it('stands down for the escape and for the pairing shell', () => {
    expect(run({ search: '?pick' })).toBeNull();
    expect(run({ pairingMeta: true })).toBeNull();
  });

  it('does nothing off the picker, or with nothing remembered', () => {
    expect(run({ base: '/demo/' })).toBeNull();
    expect(run({ base: null })).toBeNull();
    expect(run({ slug: null })).toBeNull();
  });
});
