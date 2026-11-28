import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const INDEX_HTML: string = readFileSync(resolve(here, '../../index.html'), 'utf8');

function fastPathSource(): string {
  const key = "window.lucidosWorkspaceResume = true";
  const at = INDEX_HTML.indexOf(key);
  expect(at, 'no inline script redirects to the remembered workspace').toBeGreaterThanOrEqual(0);
  const open = INDEX_HTML.lastIndexOf('<script>', at);
  const close = INDEX_HTML.indexOf('</script>', at);
  expect(open, 'unterminated script').toBeGreaterThanOrEqual(0);
  return 'var previewStoragePrefix = "";\n' + INDEX_HTML.slice(open + '<script>'.length, close);
}

interface Env {
  base?: string | null;
  pairingMeta?: boolean;
  slug?: string | null;
  search?: string;
  hash?: string;
  bouncing?: boolean;
  pairingLaunch?: boolean;
  retirement?: Promise<void>;
  standalone?: boolean;
  seen?: boolean;
  gateway?: boolean;
  navigationType?: string;
  remembered?: Map<string, string>;
  remember?: string;
}

async function run(env: Env): Promise<string | null> {
  const { base = '/~/', pairingMeta = false, slug = 'demo', search = '', hash = '', bouncing = false, pairingLaunch = false } = env;
  let replaced: string | null = null;
  const document = {
    querySelector(selector: string) {
      if (selector === 'base') return base === null ? null : { getAttribute: () => base };
      if (selector === 'meta[name="lucidos-pairing"]') return pairingMeta ? {} : null;
      if (selector === 'meta[name="lucidos-gateway-port"]') return env.gateway ? {} : null;
      return null;
    },
  };
  const localStorage = {
    getItem: (key: string) => key === 'lucidos-last-workspace' ? slug : env.remembered?.get(key) ?? null,
    setItem: (key: string, value: string) => env.remembered?.set(key, value),
  };
  const location = { pathname: base, search, hash, replace: (url: string) => void (replaced = url) };
  const window = {
    lucidosHistoryBounce: bouncing,
    lucidosPairingLaunch: pairingLaunch,
    lucidosRemoteClientReady: env.retirement,
    matchMedia: () => ({ matches: env.standalone ?? false }),
    lucidosRememberPwaWorkspace: undefined as ((route: string) => void) | undefined,
  };
  const sessionStorage = { getItem: () => env.seen ? '1' : null, setItem: () => {} };
  const performance = { getEntriesByType: () => [{ type: env.navigationType ?? 'navigate' }] };
  new Function('document', 'localStorage', 'location', 'window', 'navigator', 'sessionStorage', 'performance', fastPathSource())(
    document, localStorage, location, window, {}, sessionStorage, performance,
  );
  await env.retirement?.catch(() => {});
  await Promise.resolve();
  if (env.remember) window.lucidosRememberPwaWorkspace?.(env.remember);
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
    expect(await run({ slug: 'dev@cloud' })).toBe('/dev@cloud/');
  });

  it('stands down for a scanned pairing code', async () => {
    expect(await run({ search: '?pair=01234567' })).toBeNull();
    expect(await run({ search: '?pair=' })).toBeNull();
    expect(await run({ search: '?pair=abc&notification=n1' })).toBeNull();
  });

  it('stands down for the escape and for the pairing shell', async () => {
    expect(await run({ search: '?pick' })).toBeNull();
    expect(await run({ pairingMeta: true })).toBeNull();
  });

  it('stands down while the history bounce steps forward', async () => {
    expect(await run({ bouncing: true })).toBeNull();
  });

  it('does nothing off the picker, or with nothing remembered', async () => {
    expect(await run({ base: '/demo/' })).toBeNull();
    expect(await run({ base: null })).toBeNull();
    expect(await run({ slug: null })).toBeNull();
  });

  it('resumes the last route on picker and legacy launches, both local and remote', async () => {
    const remembered = new Map<string, string>();
    const pwa = { standalone: true, gateway: true, remembered };
    await run({ ...pwa, base: '/dev@cloud/', seen: true, remember: 'dev@cloud' });
    expect(await run(pwa)).toBe('/dev@cloud/');
    expect(await run({ ...pwa, base: '/work/' })).toBe('/dev@cloud/');
    expect(await run({ ...pwa, base: '/work/', navigationType: 'reload' })).toBe('/dev@cloud/');
    expect(await run({ ...pwa, base: '/dev@cloud/' })).toBeNull();
    await run({ ...pwa, base: '/work/', seen: true, remember: 'work' });
    expect(await run({ ...pwa, base: '/dev@cloud/' })).toBe('/work/');
  });

  it('keeps explicit workspace targets and direct-engine launches', async () => {
    const pwa = { standalone: true, gateway: true, base: '/work/' };
    expect(await run({ ...pwa, seen: true })).toBeNull();
    expect(await run({ ...pwa, seen: true, navigationType: 'reload' })).toBeNull();
    expect(await run({ ...pwa, navigationType: 'back_forward' })).toBeNull();
    expect(await run({ ...pwa, search: '?thread=abc' })).toBeNull();
    expect(await run({ ...pwa, hash: '#thread=abc' })).toBeNull();
    expect(await run({ ...pwa, gateway: false })).toBeNull();
    expect(await run({ ...pwa, standalone: false })).toBeNull();
  });

  it('only saves and resumes same-origin route segments', async () => {
    const remembered = new Map([['lucidos-pwa-workspace', '../outside']]);
    const pwa = { standalone: true, gateway: true, remembered };
    expect(await run(pwa)).toBe('/demo/');
    await run({ ...pwa, base: '/work/', seen: true, remember: 'a--b' });
    expect(await run(pwa)).toBe('/a--b/');
    await run({ ...pwa, base: '/work/', seen: true, remember: '//other.test' });
    expect(await run(pwa)).toBe('/a--b/');
    await run({ ...pwa, base: '/work/', standalone: false, remember: 'work' });
    expect(await run(pwa)).toBe('/a--b/');
  });
});
