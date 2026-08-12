/**
 * What the FOUC script actually writes onto `<html>`, driven end to end.
 *
 * This is the test the two hand-copied boot scripts never had. They were only
 * ever checked by scanning their source for literals, which is why the pair
 * could drift in behaviour while both scans passed. One program can be run
 * instead, against a fake document, so these cases pin the RESULT.
 *
 * They are also the refactor's evidence: the promise was that nothing a user
 * sees changes, and every case below states what the previous scripts did.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { applyAppearanceBoot } from './appearanceBoot';
import { configure } from '../_fetch';

interface Recorded {
  props: Record<string, string>;
  /** setProperty call order, which is what the "overrides last" rule is about. */
  order: string[];
  attrs: Record<string, string>;
  background: string;
}

let rec: Recorded;
let store: Record<string, string>;
const saved: Record<string, unknown> = {};

/**
 * Install a fake document / localStorage / matchMedia / location, and the SDK
 * base path the storage namespacing derives its slug from.
 *
 * `baseUrl` is set explicitly rather than derived from the fake DOM because
 * `_fetch.ts` resolves it once at MODULE load, before any of this runs. What
 * that derivation reads (the shell's `<base href>`, or the path before `/app/`
 * for an iframe) is `_fetch`'s own, unchanged by this script, and exercised for
 * real by `e2e/sdk-iframe-theme.spec.ts` in a browser.
 */
function setEnv(opts: {
  baseUrl?: string;
  pathname?: string;
  search?: string;
  prefersLight?: boolean;
  stored?: Record<string, string>;
}) {
  configure({ baseUrl: opts.baseUrl ?? '/myws' });
  rec = { props: {}, order: [], attrs: {}, background: '' };
  store = { ...(opts.stored ?? {}) };

  const style = {
    setProperty(name: string, value: string) {
      rec.props[name] = value;
      rec.order.push(name);
    },
    get background() { return rec.background; },
    set background(v: string) { rec.background = v; },
  };

  (globalThis as any).document = {
    documentElement: {
      style,
      setAttribute: (k: string, v: string) => { rec.attrs[k] = v; },
      getAttribute: (k: string) => rec.attrs[k] ?? null,
    },
  };
  (globalThis as any).localStorage = {
    getItem: (k: string) => (k in store ? store[k] : null),
    removeItem: (k: string) => { delete store[k]; },
  };
  (globalThis as any).matchMedia = () => ({ matches: opts.prefersLight ?? false });
  (globalThis as any).location = {
    pathname: opts.pathname ?? '/myws/',
    search: opts.search ?? '',
  };
}

beforeEach(() => {
  for (const k of ['document', 'localStorage', 'matchMedia', 'location']) {
    saved[k] = (globalThis as any)[k];
  }
});

afterEach(() => {
  for (const k of ['document', 'localStorage', 'matchMedia', 'location']) {
    (globalThis as any)[k] = saved[k];
  }
});

describe('a device with nothing stored', () => {
  it('follows the OS and paints the defaults', () => {
    setEnv({ prefersLight: true });
    const out = applyAppearanceBoot({ styleReset: true });

    expect(out.theme).toBe('system');
    expect(out.resolved).toBe('light');
    expect(rec.attrs['data-theme']).toBe('light');
    expect(rec.props['--bg-primary']).toBe('#ffffff');
    expect(rec.background).toBe('#ffffff');
    expect(rec.props['--font-ui']).toContain("'Fira Code'");
    expect(rec.props['--font-features-text']).toBe('"liga" 0, "calt" 0');
    expect(rec.props['--font-features-code']).toBe('"liga" 1, "calt" 1');
    // Nothing stored means the stylesheet's own fallback answers.
    expect(rec.props['--user-ui-scale']).toBeUndefined();
  });

  it('resolves the same default to dark on a dark OS', () => {
    setEnv({ prefersLight: false });
    applyAppearanceBoot({ styleReset: true });

    expect(rec.attrs['data-theme']).toBe('dark');
    expect(rec.props['--bg-primary']).toBe('#07172e');
  });
});

describe('stored values win', () => {
  it('an explicit theme is not second-guessed by the OS', () => {
    setEnv({
      prefersLight: true,
      stored: { 'ws:myws:lucidos-theme': 'dark' },
    });
    applyAppearanceBoot({ styleReset: true });

    expect(rec.attrs['data-theme']).toBe('dark');
  });

  it('a stored font takes its own stack and its own (absent) ligatures', () => {
    setEnv({ stored: { 'ws:myws:lucidos-font-family': 'inter' } });
    applyAppearanceBoot({ styleReset: true });

    expect(rec.props['--font-ui']).toContain("'Inter'");
    // `normal` for a font that ships no programming ligatures, so its own
    // `fi`/`fl` ligatures are left alone.
    expect(rec.props['--font-features-text']).toBe('normal');
    expect(rec.props['--font-features-code']).toBe('normal');
  });

  it('an unrecognised font falls back to the default STACK and its features together', () => {
    // The pairing is the point: a stack from one map with `normal` from the
    // other would put Fira Code's ligatures back on prose.
    setEnv({ stored: { 'ws:myws:lucidos-font-family': 'comic-sans' } });
    applyAppearanceBoot({ styleReset: true });

    expect(rec.props['--font-ui']).toContain("'Fira Code'");
    expect(rec.props['--font-features-text']).toBe('"liga" 0, "calt" 0');
  });

  it('snaps a pre-grid scale so it does not paint twice', () => {
    setEnv({ stored: { 'ws:myws:lucidos-ui-scale': '115' } });
    applyAppearanceBoot({ styleReset: true });

    expect(rec.props['--user-ui-scale']).toBe('112.5%');
  });

  it('reads the legacy enum values old devices still carry', () => {
    setEnv({ stored: { 'ws:myws:lucidos-ui-scale': 'large' } });
    applyAppearanceBoot({ styleReset: true });

    expect(rec.props['--user-ui-scale']).toBe('125%');
  });
});

describe('workspace scoping', () => {
  // The shell and its app iframes must read the SAME keys, or a value the shell
  // wrote never matches the iframe's read and every app FOUCs. The namespacing
  // itself is `_storage.ts`'s; what these pin is that the boot script goes
  // through it rather than reading raw keys.
  it('reads the per-workspace keys the shell writes', () => {
    setEnv({ baseUrl: '/myws', stored: { 'ws:myws:lucidos-theme': 'light' } });
    applyAppearanceBoot({ styleReset: true });
    expect(rec.attrs['data-theme']).toBe('light');
  });

  it('does not read the unscoped key inside a workspace', () => {
    setEnv({ baseUrl: '/myws', stored: { 'lucidos-theme': 'light' } });
    applyAppearanceBoot({ styleReset: true });
    // The unscoped value belongs to no workspace, so it must not be picked up.
    expect(rec.attrs['data-theme']).toBe('dark');
  });

  it('uses raw keys for the picker and the legacy root, which have no slug', () => {
    for (const baseUrl of ['/~', '']) {
      setEnv({ baseUrl, stored: { 'lucidos-theme': 'light' } });
      applyAppearanceBoot({ styleReset: true });
      expect(rec.attrs['data-theme']).toBe('light');
    }
  });
});

describe('the style remote', () => {
  const OVERRIDES = JSON.stringify({ '--bg-primary': '#123456', '--bad;': 'x', '--ok': 'red' });

  it('applies a valid map and drops the invalid entries', () => {
    setEnv({ stored: { 'ws:myws:lucidos-style-overrides': OVERRIDES } });
    applyAppearanceBoot({ styleReset: true });

    expect(rec.props['--ok']).toBe('red');
    expect(rec.props['--bad;']).toBeUndefined();
  });

  it('is applied LAST, so it wins over the properties above it', () => {
    setEnv({ stored: { 'ws:myws:lucidos-style-overrides': OVERRIDES } });
    applyAppearanceBoot({ styleReset: true });

    // The override of --bg-primary is the honest check: it must be the value
    // that survives, and its write must come after the theme's.
    expect(rec.props['--bg-primary']).toBe('#123456');
    expect(rec.order.lastIndexOf('--bg-primary')).toBeGreaterThan(rec.order.indexOf('--font-ui'));
  });

  it('a corrupt map never breaks first paint', () => {
    setEnv({ stored: { 'ws:myws:lucidos-style-overrides': '{oh no' } });
    applyAppearanceBoot({ styleReset: true });

    // Everything else still landed.
    expect(rec.attrs['data-theme']).toBe('dark');
    expect(rec.props['--font-ui']).toBeTruthy();
  });

  it('?style-reset clears the map in the shell', () => {
    setEnv({
      search: '?style-reset',
      stored: { 'ws:myws:lucidos-style-overrides': OVERRIDES },
    });
    applyAppearanceBoot({ styleReset: true });

    expect(rec.props['--ok']).toBeUndefined();
    expect(store['ws:myws:lucidos-style-overrides']).toBeUndefined();
  });

  it('?style-reset does NOT clear it from an iframe', () => {
    // The shell removes the key before an iframe loads, so there is nothing
    // left for that realm to clear, and an app URL that happened to carry the
    // parameter must not wipe the user's map.
    setEnv({
      pathname: '/myws/app/habit-tracker/',
      search: '?style-reset',
      stored: { 'ws:myws:lucidos-style-overrides': OVERRIDES },
    });
    applyAppearanceBoot({ styleReset: false });

    expect(rec.props['--ok']).toBe('red');
    expect(store['ws:myws:lucidos-style-overrides']).toBe(OVERRIDES);
  });
});
