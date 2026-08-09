/**
 * The live style remote's validator, and the guard that its three other realms
 * still agree with it.
 *
 * The map is written by a preference, which means any app can set it through
 * `lucidos.preferences.set` and the chat agent can set it over HTTP. It ends up
 * in `document.documentElement.style`. So this is an untrusted input path into
 * the host's own inline style, and the injection cases below are the point of
 * the file, not padding.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import {
  isValidOverrideName, isValidOverrideValue, parseStyleOverrides,
  serializeStyleOverrides, styleResetRequested,
  MAX_STYLE_OVERRIDES, MAX_STYLE_VALUE_LENGTH,
} from './styleOverrides';

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, '../../../..');

describe('override names', () => {
  it('accepts a custom property', () => {
    expect(isValidOverrideName('--accent')).toBe(true);
    expect(isValidOverrideName('--font-size-2xs')).toBe(true);
  });

  it('rejects anything that is not a custom property', () => {
    // A bare property name would let the map set `color` or `content` on the
    // root element, which is a different power from retuning a token.
    expect(isValidOverrideName('color')).toBe(false);
    expect(isValidOverrideName('-accent')).toBe(false);
    expect(isValidOverrideName('--')).toBe(false);
    expect(isValidOverrideName('--Accent')).toBe(false);
    expect(isValidOverrideName('--accent:hover')).toBe(false);
    expect(isValidOverrideName('--a}html{color')).toBe(false);
  });
});

describe('override values', () => {
  it('accepts the shapes real tokens are made of', () => {
    for (const ok of [
      '#58a6ff',
      '1.75rem',
      'rgba(255, 255, 255, 0.72)',
      'linear-gradient(180deg, #15549e 0%, #0b3868 100%)',
      'color-mix(in srgb, var(--accent) 12%, var(--bg-tertiary))',
      '0 0 0.25rem rgba(178, 206, 240, 0.42), 0 0 0.625rem rgba(158, 190, 232, 0.28)',
      'calc(100% - 12rem)',
      '16 / 9',
    ]) {
      expect(isValidOverrideValue(ok), ok).toBe(true);
    }
  });

  it('rejects a value that closes the declaration', () => {
    // The whole injection vector in one character: everything after the `;`
    // becomes a second declaration on the root element.
    expect(isValidOverrideValue('red; position: fixed')).toBe(false);
  });

  it('rejects a value that opens a rule or breaks out of a style element', () => {
    expect(isValidOverrideValue('red} html { display: none')).toBe(false);
    expect(isValidOverrideValue('red</style><script>')).toBe(false);
    expect(isValidOverrideValue('@import "http://evil.test/x.css"')).toBe(false);
  });

  it('rejects a value that fetches from another origin', () => {
    // A custom property landing in a `background` issues the request, which
    // tells the value's author the page was viewed.
    expect(isValidOverrideValue('url(http://evil.test/pixel.png)')).toBe(false);
    expect(isValidOverrideValue('URL( http://evil.test/x )')).toBe(false);
    expect(isValidOverrideValue('image-set("http://evil.test/x.png" 1x)')).toBe(false);
    expect(isValidOverrideValue('expression(alert(1))')).toBe(false);
  });

  it('rejects escapes, comments, and oversize or empty values', () => {
    // `\3b` is a semicolon that a naive scan for `;` would miss.
    expect(isValidOverrideValue('red\\3b position:fixed')).toBe(false);
    expect(isValidOverrideValue('red /* swallow the rest')).toBe(false);
    expect(isValidOverrideValue('')).toBe(false);
    expect(isValidOverrideValue('   ')).toBe(false);
    expect(isValidOverrideValue('a'.repeat(MAX_STYLE_VALUE_LENGTH + 1))).toBe(false);
    expect(isValidOverrideValue('a'.repeat(MAX_STYLE_VALUE_LENGTH))).toBe(true);
  });
});

describe('parseStyleOverrides', () => {
  it('round-trips a valid map', () => {
    const map = { '--accent': '#ff0000', '--mark-size': '1.8rem' };
    expect(parseStyleOverrides(serializeStyleOverrides(map))).toEqual(map);
  });

  it('drops invalid entries instead of failing the whole map', () => {
    // One bad value written by an app must not cost the user every value they
    // tuned by hand.
    const raw = JSON.stringify({
      '--accent': '#ff0000',
      'color': 'red',
      '--bad': 'red; position: fixed',
      '--n': 12,
    });
    expect(parseStyleOverrides(raw)).toEqual({ '--accent': '#ff0000' });
  });

  it('trims values', () => {
    expect(parseStyleOverrides('{"--accent":"  #fff  "}')).toEqual({ '--accent': '#fff' });
  });

  it('returns an empty map for junk rather than throwing', () => {
    for (const junk of ['', null, undefined, 'not json', '[]', '"str"', '42', 'null']) {
      expect(parseStyleOverrides(junk as string)).toEqual({});
    }
  });

  it('caps the number of entries', () => {
    const big: Record<string, string> = {};
    for (let i = 0; i < MAX_STYLE_OVERRIDES + 50; i++) big[`--t${i}`] = '1px';
    expect(Object.keys(parseStyleOverrides(JSON.stringify(big))).length).toBe(MAX_STYLE_OVERRIDES);
  });
});

describe('styleResetRequested', () => {
  it('matches the escape hatch in its real forms', () => {
    // This is the way out of a value that made the UI unusable, so the match
    // has to be forgiving about where in the query string it lands.
    expect(styleResetRequested('?style-reset')).toBe(true);
    expect(styleResetRequested('?a=1&style-reset')).toBe(true);
    expect(styleResetRequested('?style-reset=1')).toBe(true);
    expect(styleResetRequested('?style-reset&b=2')).toBe(true);
  });

  it('does not match a lookalike', () => {
    expect(styleResetRequested('')).toBe(false);
    expect(styleResetRequested('?style-resets=1')).toBe(false);
    expect(styleResetRequested('?not-style-reset')).toBe(false);
  });
});

describe('the other realms mirror this validator', () => {
  // Four realms apply this map and none can import another's copy: the app
  // module here, the index.html boot script (runs before any module), the
  // engine-served sdk-prefs.js (iframe first paint), and the SDK (a separate
  // package, iframe live updates). A rule relaxed in one realm is a hole in all
  // of them, so the literals are pinned here. The Rust side pins its own in
  // `api/sdk_prefs.rs`.
  const NAME_RE_SRC = '/^--[a-z][a-z0-9-]*$/';
  const VALUE_RE_SRC = String.raw`/[;{}<>@\\]|url\s*\(|image-set\s*\(|expression\s*\(|\/\*/i`;

  const realms: Array<[string, string]> = [
    ['index.html boot script', resolve(REPO_ROOT, 'crates/lucidos-app/index.html')],
    ['engine sdk-prefs.js', resolve(REPO_ROOT, 'crates/lucidos-engine/src/api/sdk_prefs.rs')],
    ['SDK ui.ts', resolve(REPO_ROOT, 'packages/lucidos-sdk/src/ui.ts')],
  ];

  for (const [label, path] of realms) {
    it(`${label} uses the same name and value rules`, () => {
      const src = readFileSync(path, 'utf8');
      expect(src, `${label}: name rule`).toContain(NAME_RE_SRC);
      expect(src, `${label}: banned-value rule`).toContain(VALUE_RE_SRC);
      expect(src, `${label}: entry cap`).toContain('200');
      expect(src, `${label}: value length cap`).toContain('120');
    });
  }

  it('the boot script applies overrides after theme, font and scale', () => {
    // Order is load-bearing: the remote may override --bg-primary, --font-ui
    // and --user-ui-scale, and inline properties are last-write-wins.
    const src = readFileSync(resolve(REPO_ROOT, 'crates/lucidos-app/index.html'), 'utf8');
    const overrides = src.indexOf('lucidos-style-overrides');
    expect(overrides).toBeGreaterThan(-1);
    for (const earlier of ['lucidos-theme', 'lucidos-font-family', 'lucidos-ui-scale']) {
      expect(src.indexOf(earlier), earlier).toBeLessThan(overrides);
    }
  });

  it('the boot script scopes its storage key per workspace', () => {
    // The gateway serves every workspace from one origin (ADR 0014), so an
    // unscoped key would bleed one workspace's tuning into another's.
    const src = readFileSync(resolve(REPO_ROOT, 'crates/lucidos-app/index.html'), 'utf8');
    expect(src).toContain("wsKey('lucidos-style-overrides')");
  });
});
