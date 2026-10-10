/**
 * The theme half of the appearance contract: what every surface does with the
 * resolved maps the engine serves.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import {
  EMPTY_THEME,
  SHADOW_OVERRIDE_TOKENS,
  isReservedOverrideName,
  isValidOverrideName,
  isValidOverrideValue,
  themeBackground,
  themeTokenNames,
  parseResolvedTheme,
  parseStyleOverrides,
  replaceInlineTokens,
  shadowWithinReach,
} from './appearance';

const here: string = dirname(fileURLToPath(import.meta.url));
const cases: {
  shadowTokens: string[];
  shadowValuesWithinReach: string[];
  shadowValuesPastReach: string[];
  uiFontTokens: string[];
  validNames: string[];
  invalidNames: string[];
  validValues: string[];
  invalidValues: string[];
} = JSON.parse(readFileSync(
  resolve(here, '../../../crates/lucidos-engine/src/core/themes/theme-validation-cases.json'),
  'utf-8',
));

describe('the token gate agrees with the engine', () => {
  // The engine's `core/themes` tests read the same fixture. If the two sides
  // disagreed, a theme could save and then never paint.
  it.each(cases.validNames)('accepts the name %s', name => {
    expect(isValidOverrideName(name)).toBe(true);
  });
  it.each(cases.invalidNames)('refuses the name %s', name => {
    expect(isValidOverrideName(name)).toBe(false);
  });
  it.each(cases.validValues)('accepts the value %s', value => {
    expect(isValidOverrideValue(value)).toBe(true);
  });
  it.each(cases.invalidValues)('refuses the value %s', value => {
    expect(isValidOverrideValue(value)).toBe(false);
  });
});

describe('parseResolvedTheme', () => {
  it('reads both modes', () => {
    const theme = parseResolvedTheme(JSON.stringify({
      dark: { '--accent': '#88c0d0' },
      light: { '--accent': '#5e81ac' },
    }));
    expect(theme).toEqual({
      dark: { '--accent': '#88c0d0' },
      light: { '--accent': '#5e81ac' },
      fonts: {},
      workspace_fonts: [],
    });
  });

  it('drops an entry the gate refuses and keeps the rest', () => {
    const theme = parseResolvedTheme(JSON.stringify({
      dark: { '--accent': 'url(https://example.com/x)', '--bg-primary': '#101010' },
    }));
    expect(theme).toEqual({
      dark: { '--bg-primary': '#101010' }, light: {}, fonts: {}, workspace_fonts: [],
    });
  });

  it('reads the fonts a theme suggests', () => {
    const theme = parseResolvedTheme(JSON.stringify({
      dark: {}, light: {}, fonts: { ui: 'geist', mono: 'geist-mono' },
    }));
    expect(theme.fonts).toEqual({ ui: 'geist', mono: 'geist-mono' });
  });

  // The engine refuses these when the theme is written. A cache in storage is
  // data, so the client drops them too.
  it('drops an unknown id and an inherited key', () => {
    for (const ui of ['comic-sans', 'toString', 42]) {
      expect(parseResolvedTheme(JSON.stringify({ fonts: { ui, mono: ui } })).fonts).toEqual({});
    }
  });

  // ADR 0303: the fonts that once loaded from Google are bundled, so a theme
  // may name them.
  it('keeps a font that once loaded from Google', () => {
    const theme = parseResolvedTheme(JSON.stringify({ fonts: { ui: 'inter', mono: 'jetbrains-mono' } }));
    expect(theme.fonts).toEqual({ ui: 'inter', mono: 'jetbrains-mono' });
  });

  it.each([null, undefined, '', 'not json', '[]', '"x"', '{"dark":[1]}'])(
    'parses %s to the empty theme',
    raw => {
      expect(parseResolvedTheme(raw)).toEqual(EMPTY_THEME);
    },
  );
});

describe('themeBackground', () => {
  it('takes a hex literal', () => {
    expect(themeBackground({ '--bg-primary': '#2e3440' })).toBe('#2e3440');
    expect(themeBackground({ '--bg-primary': '#fff' })).toBe('#fff');
  });

  it.each(['var(--x)', 'color-mix(in oklab, red 50%, blue)', 'rgb(0, 0, 0)', 'red'])(
    'leaves %s to the stylesheet, since it cannot paint before CSS resolves',
    bg => {
      expect(themeBackground({ '--bg-primary': bg })).toBeNull();
    },
  );

  it('is null when the theme sets no background', () => {
    expect(themeBackground({})).toBeNull();
  });
});

describe('replaceInlineTokens', () => {
  function recorder() {
    const props = new Map<string, string>();
    return {
      props,
      style: {
        setProperty: (name: string, value: string) => { props.set(name, value); },
        removeProperty: (name: string) => { props.delete(name); },
      },
    };
  }

  it('lays the new map and removes what it dropped', () => {
    const { props, style } = recorder();
    let names = replaceInlineTokens(style, [], { '--a': '1', '--b': '2' });
    names = replaceInlineTokens(style, names, { '--a': '3' });
    expect(Object.fromEntries(props)).toEqual({ '--a': '3' });
    expect(names).toEqual(['--a']);
  });

  it('a dropped name uncovers the layer beneath instead of vanishing', () => {
    const { props, style } = recorder();
    const theme = { '--accent': '#88c0d0' };
    props.set('--accent', theme['--accent']);
    const names = replaceInlineTokens(style, [], { '--accent': '#ff0000' }, theme);
    replaceInlineTokens(style, names, {}, theme);
    expect(props.get('--accent')).toBe('#88c0d0');
  });
});

describe('themeTokenNames', () => {
  it('lists every name either mode sets, once', () => {
    expect(themeTokenNames({ dark: { '--a': '1', '--b': '2' }, light: { '--a': '3' } }).sort())
      .toEqual(['--a', '--b']);
  });
});

describe('protected surfaces (ADR 0309)', () => {
  it('lays the engine-computed protected palette from a theme map', () => {
    const theme = parseResolvedTheme(JSON.stringify({
      dark: { '--accent': '#88c0d0', '--protected-text': '#eceff4' },
      light: {},
    }));
    expect(theme.dark['--protected-text']).toBe('#eceff4');
  });

  it('drops reserved names from a style override map, any app being able to write one', () => {
    const overrides = parseStyleOverrides(JSON.stringify({
      '--accent': '#ff0000',
      '--protected-text': '#000000',
      '--protected-confirm': '#ff0000',
      '--z-modal': '0',
      '--z-toast': '99999',
      '--font-ui': 'Wingdings',
      '--font': 'Wingdings',
      '--font-features-code': '"ss01"',
      '--user-ui-scale': '5%',
    }));
    expect(overrides).toEqual({ '--accent': '#ff0000' });
  });

  it('drops the scanlines from a style override, since the protected palette is clamped against them', () => {
    const scanlines = 'repeating-linear-gradient(transparent 0, #0000001a 2px)';
    const overrides = parseStyleOverrides(JSON.stringify({
      '--part-screen-background-image': scanlines,
      '--part-surface-border-style': 'double',
    }));
    expect(overrides).toEqual({ '--part-surface-border-style': 'double' });
    // A theme carries them: the resolved map keeps the token.
    const theme = parseResolvedTheme(JSON.stringify({ dark: { '--part-screen-background-image': scanlines }, light: {} }));
    expect(theme.dark['--part-screen-background-image']).toBe(scanlines);
  });

  it.each(cases.uiFontTokens)('refuses the UI font token %s in an override, as the engine does in a theme', name => {
    expect(isReservedOverrideName(name)).toBe(true);
  });

  it('keeps the code font and the type scale overridable outside protected surfaces', () => {
    for (const name of ['--font-mono', '--font-size-md', '--space-md', '--focus-ring', '--part-surface-border-width']) {
      expect(isReservedOverrideName(name), name).toBe(false);
    }
  });
});

describe('a shadow override stays within reach of its box, as a theme shadow does', () => {
  it('names the same shadow tokens the engine catalog does', () => {
    expect([...SHADOW_OVERRIDE_TOKENS]).toEqual(cases.shadowTokens);
  });
  it.each(cases.shadowValuesWithinReach)('accepts %s', value => {
    expect(shadowWithinReach(value)).toBe(true);
  });
  it.each(cases.shadowValuesPastReach)('refuses %s', value => {
    expect(shadowWithinReach(value)).toBe(false);
  });
  it('drops an out-of-reach shadow from an override map', () => {
    expect(parseStyleOverrides(JSON.stringify({
      '--shadow-lg': '0 0 0 100vmax #000',
      '--shadow-sm': '0 2px 6px rgba(0, 0, 0, 0.2)',
    }))).toEqual({ '--shadow-sm': '0 2px 6px rgba(0, 0, 0, 0.2)' });
  });
});
