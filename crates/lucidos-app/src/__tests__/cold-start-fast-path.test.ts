/**
 * The picker's cold-start fast path, run for real.
 *
 * It is an inline `<head>` script in `index.html`, so nothing imports it and a
 * source scan is the usual fallback. Here the script is lifted out and called
 * with stub globals instead, which is worth the extraction: every rule in it is
 * a stand-down, and a scan can only prove the words are present.
 *
 * A pairing launch stays on the picker after the head capture strips its URL.
 * The captured flag protects both fragment links and legacy query input.
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
  /** Has the history bounce script above already stepped forward? */
  bouncing?: boolean;
  pairingLaunch?: boolean;
  retirement?: Promise<void>;
}

/** Run the fast path against `env` and answer the URL it replaced with, if any. */
async function run(env: Env): Promise<string | null> {
  const { base = '/~/', pairingMeta = false, slug = 'demo', search = '', hash = '', bouncing = false, pairingLaunch = false } = env;
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
  const window = { lucidosHistoryBounce: bouncing, lucidosPairingLaunch: pairingLaunch, lucidosRemoteClientReady: env.retirement };
  // The script under test is inline HTML, so only `new Function` can run it.
  // eslint-disable-next-line no-new-func
  new Function('document', 'localStorage', 'location', 'window', fastPathSource())(
    document,
    localStorage,
    location,
    window,
  );
  await env.retirement?.catch(() => {});
  await Promise.resolve();
  return replaced;
}

describe('picker cold-start fast path', () => {
  it('waits for retirement and stays at trusted entry on failure', async () => {
    let release!: () => void;
    const retirement = new Promise<void>(resolve => { release = resolve; });
    let finished = false;
    const pending = run({ slug: 'work@peer', retirement }).then(url => { finished = true; return url; });
    await Promise.resolve();
    expect(finished).toBe(false);
    release();
    expect(await pending).toBe('/work@peer/');
    expect(await run({ slug: 'work@peer', retirement: Promise.reject(new Error('retirement failed')) })).toBeNull();
  });
  it('opens the remembered workspace, carrying the query and hash', async () => {
    expect(await run({})).toBe('/demo/');
    expect(await run({ search: '?notification=n1', hash: '#thread=t1' })).toBe(
      '/demo/?notification=n1#thread=t1',
    );
  });

  it('stays on a pairing launch after the head script erased its URL', async () => {
    expect(await run({ pairingLaunch: true })).toBeNull();
    expect(await run({ hash: '#pair=01234567' })).toBeNull();
  });

  it('opens a remembered remote workspace with its @ intact', async () => {
    // The gateway reads `<slug>@<name>` off the raw path, so `%40` misses it.
    expect(await run({ slug: 'dev@cloud' })).toBe('/dev@cloud/');
  });

  it('stands down for a scanned pairing code', async () => {
    // The whole point: `/~/` is public and stamps the code into the manifest,
    // `/<slug>/` is gated and cannot.
    expect(await run({ search: '?pair=01234567' })).toBeNull();
    // Whatever the value looks like. The gateway owns the grammar, and a
    // redirect helps no pairing navigation.
    expect(await run({ search: '?pair=' })).toBeNull();
    expect(await run({ search: '?pair=abc&notification=n1' })).toBeNull();
  });

  it('stands down for the escape and for the pairing shell', async () => {
    expect(await run({ search: '?pick' })).toBeNull();
    expect(await run({ pairingMeta: true })).toBeNull();
  });

  it('stands down while the history bounce steps forward', async () => {
    // A redirect scheduled now would win over the pending step forward, and
    // replace the stale picker entry with a second copy of the workspace.
    expect(await run({ bouncing: true })).toBeNull();
  });

  it('does nothing off the picker, or with nothing remembered', async () => {
    expect(await run({ base: '/demo/' })).toBeNull();
    expect(await run({ base: null })).toBeNull();
    expect(await run({ slug: null })).toBeNull();
  });
});
