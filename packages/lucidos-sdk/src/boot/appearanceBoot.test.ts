import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { applyAppearanceBoot, type BootOptions } from './appearanceBoot';
import { configure } from '../_fetch';

const SHELL: BootOptions = {
  styleReset: true,
  durationScale: true,
};
const IFRAME: BootOptions = {
  styleReset: false,
  durationScale: false,
};

interface Recorded {
  props: Record<string, string>;
  order: string[];
  attrs: Record<string, string>;
  background: string;
}

let rec: Recorded;
let store: Record<string, string>;
const saved: Record<string, unknown> = {};

function setEnv(opts: {
  baseUrl?: string;
  pathname?: string;
  search?: string;
  prefersLight?: boolean;
  osReduces?: boolean;
  osReducesTransparency?: boolean;
  osMoreContrast?: boolean;
  stored?: Record<string, string>;
  served?: Record<string, string>;
}) {
  configure({ baseUrl: opts.baseUrl ?? '/myws' });
  if (opts.served) (globalThis as any).__lucidosPrefs = opts.served;
  else delete (globalThis as any).__lucidosPrefs;
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
    querySelector: () => null,
    documentElement: {
      style,
      setAttribute: (k: string, v: string) => { rec.attrs[k] = v; },
      getAttribute: (k: string) => rec.attrs[k] ?? null,
    },
  };
  (globalThis as any).localStorage = {
    getItem: (k: string) => (k in store ? store[k] : null),
    setItem: (k: string, v: string) => { store[k] = v; },
    removeItem: (k: string) => { delete store[k]; },
  };
  (globalThis as any).matchMedia = (query: string) => ({
    matches: query.includes('reduced-motion') ? opts.osReduces ?? false
      : query.includes('reduced-transparency') ? opts.osReducesTransparency ?? false
        : query.includes('prefers-contrast') ? opts.osMoreContrast ?? false
          : opts.prefersLight ?? false,
  });
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
  delete (globalThis as any).__lucidosPrefs;
});

describe('an isolated app frame, served its values', () => {
  it('paints the served appearance with nothing in storage', () => {
    setEnv({
      prefersLight: true,
      served: { 'theme-mode': 'dark', 'font-family': 'inter', 'ui-scale': '150' },
    });
    applyAppearanceBoot(IFRAME);

    expect(rec.attrs['data-theme-mode']).toBe('dark');
    expect(rec.props['--font-ui']).toContain("'Inter'");
    expect(rec.props['--user-ui-scale']).toBe('150%');
  });

  it('the served value beats a stale stored one', () => {
    setEnv({
      served: { 'theme-mode': 'light' },
      stored: { 'ws:myws:lucidos-theme-mode': 'dark' },
    });
    applyAppearanceBoot(IFRAME);

    expect(rec.attrs['data-theme-mode']).toBe('light');
  });

  it('a key the engine did not resolve falls back to storage', () => {
    setEnv({
      served: { 'theme-mode': 'dark' },
      stored: { 'ws:myws:lucidos-ui-scale': '125' },
    });
    applyAppearanceBoot(IFRAME);

    expect(rec.attrs['data-theme-mode']).toBe('dark');
    expect(rec.props['--user-ui-scale']).toBe('125%');
  });

  it('reads the pre-grid scale aliases in the same order the live re-apply does', () => {
    setEnv({ served: { 'text-size': 'large' } });
    applyAppearanceBoot(IFRAME);

    expect(rec.props['--user-ui-scale']).toBe('125%');
  });

  it('an empty served value is not a value, and storage answers', () => {
    setEnv({
      served: { 'theme-mode': '' },
      stored: { 'ws:myws:lucidos-theme-mode': 'light' },
    });
    applyAppearanceBoot(IFRAME);

    expect(rec.attrs['data-theme-mode']).toBe('light');
  });
});

describe('a device with nothing stored', () => {
  it('follows the OS and paints the defaults', () => {
    setEnv({ prefersLight: true });
    const out = applyAppearanceBoot(SHELL);

    expect(out.mode).toBe('system');
    expect(out.resolved).toBe('light');
    expect(rec.attrs['data-theme-mode']).toBe('light');
    expect(rec.props['--bg-primary']).toBe('#ffffff');
    expect(rec.background).toBe('#ffffff');
    expect(rec.props['--font-ui']).toContain("'Fira Code'");
    expect(rec.props['--font-features-text']).toBe('"liga" 0, "calt" 0');
    expect(rec.props['--font-features-code']).toBe('"liga" 1, "calt" 1');
    expect(rec.props['--user-ui-scale']).toBeUndefined();
  });

  it('resolves the same default to dark on a dark OS', () => {
    setEnv({ prefersLight: false });
    applyAppearanceBoot(SHELL);

    expect(rec.attrs['data-theme-mode']).toBe('dark');
    expect(rec.props['--bg-primary']).toBe('#07172e');
  });
});

describe('stored values win', () => {
  it('an explicit theme is not second-guessed by the OS', () => {
    setEnv({
      prefersLight: true,
      stored: { 'ws:myws:lucidos-theme-mode': 'dark' },
    });
    applyAppearanceBoot(SHELL);

    expect(rec.attrs['data-theme-mode']).toBe('dark');
  });

  it('a stored font takes its own stack and its own (absent) ligatures', () => {
    setEnv({ stored: { 'ws:myws:lucidos-font-family': 'inter' } });
    applyAppearanceBoot(SHELL);

    expect(rec.props['--font-ui']).toContain("'Inter'");
    expect(rec.props['--font-features-text']).toBe('normal');
    expect(rec.props['--font-features-code']).toBe('normal');
  });

  it('an unrecognised font falls back to the default STACK and its features together', () => {
    setEnv({ stored: { 'ws:myws:lucidos-font-family': 'comic-sans' } });
    applyAppearanceBoot(SHELL);

    expect(rec.props['--font-ui']).toContain("'Fira Code'");
    expect(rec.props['--font-features-text']).toBe('"liga" 0, "calt" 0');
  });

  it('snaps a pre-grid scale so it does not paint twice', () => {
    setEnv({ stored: { 'ws:myws:lucidos-ui-scale': '115' } });
    applyAppearanceBoot(SHELL);

    expect(rec.props['--user-ui-scale']).toBe('112.5%');
  });

  it('reads the legacy enum values old devices still carry', () => {
    setEnv({ stored: { 'ws:myws:lucidos-ui-scale': 'large' } });
    applyAppearanceBoot(SHELL);

    expect(rec.props['--user-ui-scale']).toBe('125%');
  });
});

describe('workspace scoping', () => {
  it('reads the per-workspace keys the shell writes', () => {
    setEnv({ baseUrl: '/myws', stored: { 'ws:myws:lucidos-theme-mode': 'light' } });
    applyAppearanceBoot(SHELL);
    expect(rec.attrs['data-theme-mode']).toBe('light');
  });

  it('does not read the unscoped key inside a workspace', () => {
    setEnv({ baseUrl: '/myws', stored: { 'lucidos-theme-mode': 'light' } });
    applyAppearanceBoot(SHELL);
    expect(rec.attrs['data-theme-mode']).toBe('dark');
  });

  it('uses raw keys for the picker and the legacy root, which have no slug', () => {
    for (const baseUrl of ['/~', '']) {
      setEnv({ baseUrl, stored: { 'lucidos-theme-mode': 'light' } });
      applyAppearanceBoot(SHELL);
      expect(rec.attrs['data-theme-mode']).toBe('light');
    }
  });
});

describe('the theme', () => {
  const LOOK = JSON.stringify({
    dark: { '--bg-primary': '#2e3440', '--accent': '#88c0d0', '--leak': 'url(https://example.com)' },
    light: { '--bg-primary': 'var(--x)', '--accent': '#5e81ac' },
  });

  it('paints the map for the resolved mode, background included', () => {
    setEnv({ stored: { 'ws:myws:lucidos-theme-resolved': LOOK } });
    applyAppearanceBoot(SHELL);

    expect(rec.props['--accent']).toBe('#88c0d0');
    expect(rec.props['--bg-primary']).toBe('#2e3440');
    expect(rec.background).toBe('#2e3440');
    expect(rec.props['--leak']).toBeUndefined();
  });

  it('paints a background that is not a hex literal through the var', () => {
    setEnv({ prefersLight: true, stored: { 'ws:myws:lucidos-theme-resolved': LOOK } });
    applyAppearanceBoot(SHELL);

    expect(rec.props['--accent']).toBe('#5e81ac');
    expect(rec.props['--bg-primary']).toBe('var(--x)');
    expect(rec.background).toBe('var(--bg-primary)');
  });

  it('an isolated app frame takes the map the engine served', () => {
    setEnv({ served: { theme_resolved: LOOK } });
    applyAppearanceBoot(IFRAME);

    expect(rec.props['--accent']).toBe('#88c0d0');
  });

  it('sits under the style remote, so an override still wins', () => {
    setEnv({
      stored: {
        'ws:myws:lucidos-theme-resolved': LOOK,
        'ws:myws:lucidos-style-overrides': JSON.stringify({ '--accent': '#ff0000' }),
      },
    });
    applyAppearanceBoot(SHELL);

    expect(rec.props['--accent']).toBe('#ff0000');
  });

  it('?style-reset drops the theme too, the way out of an unreadable one', () => {
    setEnv({ search: '?style-reset', stored: { 'ws:myws:lucidos-theme-resolved': LOOK } });
    applyAppearanceBoot(SHELL);

    expect(rec.props['--accent']).toBeUndefined();
    expect(store['ws:myws:lucidos-theme-resolved']).toBeUndefined();
  });

  it('?style-reset leaves no protected token inline, so protected surfaces paint the stylesheet defaults', () => {
    const withPalette = JSON.stringify({
      dark: { '--accent': '#88c0d0', '--protected-text': '#eceff4', '--protected-confirm': '#2f7a3a' },
      light: {},
    });
    setEnv({ search: '?style-reset', stored: { 'ws:myws:lucidos-theme-resolved': withPalette } });
    applyAppearanceBoot(SHELL);

    expect(Object.keys(rec.props).filter(name => name.startsWith('--protected-'))).toEqual([]);
  });

  it('paints the protected palette a theme carries, but never one an override names', () => {
    setEnv({
      stored: {
        'ws:myws:lucidos-theme-resolved': JSON.stringify({ dark: { '--protected-text': '#eceff4' }, light: {} }),
        'ws:myws:lucidos-style-overrides': JSON.stringify({ '--protected-text': '#000000' }),
      },
    });
    applyAppearanceBoot(SHELL);

    expect(rec.props['--protected-text']).toBe('#eceff4');
  });

  it('a corrupt cache paints no theme and breaks nothing', () => {
    setEnv({ stored: { 'ws:myws:lucidos-theme-resolved': '{oh no' } });
    applyAppearanceBoot(SHELL);

    expect(rec.props['--accent']).toBeUndefined();
    expect(rec.background).toBe('#07172e');
  });
});

describe('the font a theme suggests', () => {
  const GEIST_THEME = JSON.stringify({ dark: {}, light: {}, fonts: { ui: 'geist' } });

  it('a device with no font set paints the theme font', () => {
    setEnv({ stored: { 'ws:myws:lucidos-theme-resolved': GEIST_THEME } });
    applyAppearanceBoot(SHELL);
    expect(rec.props['--font-ui']).toMatch(/^'Geist',/);
    expect(rec.props['--font-features-text']).toBe('normal');
  });

  it('`theme` paints the theme font too', () => {
    setEnv({
      stored: { 'ws:myws:lucidos-theme-resolved': GEIST_THEME, 'ws:myws:lucidos-font-family': 'theme' },
    });
    applyAppearanceBoot(SHELL);
    expect(rec.props['--font-ui']).toMatch(/^'Geist',/);
  });

  it('an explicit pick wins over the theme font', () => {
    setEnv({
      stored: { 'ws:myws:lucidos-theme-resolved': GEIST_THEME, 'ws:myws:lucidos-font-family': 'inter' },
    });
    applyAppearanceBoot(SHELL);
    expect(rec.props['--font-ui']).toContain("'Inter'");
  });

  it('an isolated app frame resolves against the theme the engine served', () => {
    setEnv({ served: { theme_resolved: GEIST_THEME } });
    applyAppearanceBoot(IFRAME);
    expect(rec.props['--font-ui']).toMatch(/^'Geist',/);
  });

  it('a theme with no font falls back to Fira Code', () => {
    setEnv({ stored: { 'ws:myws:lucidos-font-family': 'theme' } });
    applyAppearanceBoot(SHELL);
    expect(rec.props['--font-ui']).toContain("'Fira Code'");
  });

  it('a forged cache naming an unknown font paints the fallback', () => {
    const forged = JSON.stringify({ dark: {}, light: {}, fonts: { ui: 'comic-sans' } });
    setEnv({ stored: { 'ws:myws:lucidos-theme-resolved': forged } });
    applyAppearanceBoot(SHELL);
    expect(rec.props['--font-ui']).toContain("'Fira Code'");
  });

  it('a theme may suggest a font that once loaded from Google', () => {
    const theme = JSON.stringify({ dark: {}, light: {}, fonts: { ui: 'inter' } });
    setEnv({ stored: { 'ws:myws:lucidos-theme-resolved': theme } });
    applyAppearanceBoot(SHELL);
    expect(rec.props['--font-ui']).toMatch(/^'Inter',/);
  });
});

describe('the style remote', () => {
  const OVERRIDES = JSON.stringify({ '--bg-primary': '#123456', '--bad;': 'x', '--ok': 'red' });

  it('applies a valid map and drops the invalid entries', () => {
    setEnv({ stored: { 'ws:myws:lucidos-style-overrides': OVERRIDES } });
    applyAppearanceBoot(SHELL);

    expect(rec.props['--ok']).toBe('red');
    expect(rec.props['--bad;']).toBeUndefined();
  });

  it('is applied LAST, so it wins over the properties above it', () => {
    setEnv({ stored: { 'ws:myws:lucidos-style-overrides': OVERRIDES } });
    applyAppearanceBoot(SHELL);

    expect(rec.props['--bg-primary']).toBe('#123456');
    expect(rec.order.lastIndexOf('--bg-primary')).toBeGreaterThan(rec.order.indexOf('--font-ui'));
  });

  it('a corrupt map never breaks first paint', () => {
    setEnv({ stored: { 'ws:myws:lucidos-style-overrides': '{oh no' } });
    applyAppearanceBoot(SHELL);

    expect(rec.attrs['data-theme-mode']).toBe('dark');
    expect(rec.props['--font-ui']).toBeTruthy();
  });

  it('?style-reset clears the map in the shell', () => {
    setEnv({
      search: '?style-reset',
      stored: { 'ws:myws:lucidos-style-overrides': OVERRIDES },
    });
    applyAppearanceBoot(SHELL);

    expect(rec.props['--ok']).toBeUndefined();
    expect(store['ws:myws:lucidos-style-overrides']).toBeUndefined();
  });

  it('?style-reset does NOT clear it from an iframe', () => {
    setEnv({
      pathname: '/myws/app/habit-tracker/',
      search: '?style-reset',
      stored: { 'ws:myws:lucidos-style-overrides': OVERRIDES },
    });
    applyAppearanceBoot(IFRAME);

    expect(rec.props['--ok']).toBe('red');
    expect(store['ws:myws:lucidos-style-overrides']).toBe(OVERRIDES);
  });
});

describe('motion', () => {
  const MOTION = 'ws:myws:lucidos-motion';

  it('resolves every stored value against the OS before first paint', () => {
    const cases: Array<[string | undefined, boolean, string]> = [
      [undefined, false, 'full'],
      [undefined, true, 'reduce'],
      ['system', true, 'reduce'],
      ['reduce', false, 'reduce'],
      ['full', true, 'full'],
      ['bogus', true, 'reduce'],
    ];
    for (const [stored, osReduces, expected] of cases) {
      setEnv({ osReduces, stored: stored ? { [MOTION]: stored } : {} });
      const out = applyAppearanceBoot(SHELL);
      expect(rec.attrs['data-motion'], `${stored} with OS ${osReduces}`).toBe(expected);
      expect(out.reducedMotion).toBe(expected === 'reduce');
    }
  });

  it('an isolated app frame takes the served value', () => {
    setEnv({ osReduces: false, served: { motion: 'reduce' } });
    applyAppearanceBoot(IFRAME);
    expect(rec.attrs['data-motion']).toBe('reduce');
  });

  it('publishes the slider scale in the shell, collapsed under reduced motion', () => {
    setEnv({ stored: { 'ws:myws:lucidos-animation-speed-slider': '-10' } });
    applyAppearanceBoot(SHELL);
    expect(Number(rec.props['--duration-scale'])).toBeCloseTo(10, 10);

    setEnv({ stored: { 'ws:myws:lucidos-animation-speed-slider': '-10', [MOTION]: 'reduce' } });
    applyAppearanceBoot(SHELL);
    expect(rec.props['--duration-scale']).toBe('0.001');
  });

  it('leaves the scale alone in an app frame, whose stylesheet pins it', () => {
    setEnv({ served: { motion: 'reduce' } });
    applyAppearanceBoot(IFRAME);
    expect(rec.props['--duration-scale']).toBeUndefined();
  });
});

describe('theme effects', () => {
  const EFFECTS = 'ws:myws:lucidos-theme-effects';

  it('resolves system from both media queries and a stored value before first paint', () => {
    const cases: Array<[string | undefined, boolean, boolean, string]> = [
      [undefined, false, false, 'full'],
      [undefined, true, false, 'reduce'],
      ['system', false, true, 'reduce'],
      ['reduce', false, false, 'reduce'],
      ['full', true, true, 'full'],
      ['bogus', false, true, 'reduce'],
    ];
    for (const [stored, transparency, contrast, expected] of cases) {
      setEnv({
        osReducesTransparency: transparency,
        osMoreContrast: contrast,
        stored: stored ? { [EFFECTS]: stored } : {},
      });
      applyAppearanceBoot(SHELL);
      expect(rec.attrs['data-theme-effects'], `${stored}, ${transparency}, ${contrast}`).toBe(expected);
    }
  });

  it('an isolated app frame takes the served value', () => {
    setEnv({ served: { 'theme-effects': 'reduce' } });
    applyAppearanceBoot(IFRAME);
    expect(rec.attrs['data-theme-effects']).toBe('reduce');
  });
});
