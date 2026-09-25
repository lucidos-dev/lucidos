/**
 * The appearance contract, pinned by input table.
 *
 * Written BEFORE the four duplicated copies were collapsed into it, asserting
 * what they did at that moment. The refactor's whole promise is that nothing a
 * user sees changes, so these cases are the fixed target the rewritten boot
 * scripts and store have to keep hitting: unset, each theme, each font, a
 * legacy scale value, and garbage in every slot.
 */
import { describe, it, expect, vi } from 'vitest';
import {
  DEFAULT_FONT_FAMILY,
  DEFAULT_MOTION,
  DEFAULT_THEME,
  FONT_FAMILY_VALUES,
  FONT_FEATURES_DEFAULT,
  MOTION_PREFS,
  REDUCED_MOTION_DURATION_SCALE,
  THEME_BG,
  UI_SCALE_DEFAULT,
  clampUiScale,
  durationScaleFor,
  fontFeaturesFor,
  parseAnimationSpeed,
  parseMotion,
  parseUiScale,
  resolveReducedMotion,
  resolveFontKey,
  resolveTheme,
  resolveThemePreference,
  type FontFamily,
} from './appearance';

describe('theme preference precedence', () => {
  it('prefers a valid server value over everything else', () => {
    expect(resolveThemePreference('light', 'dark', () => 'dark')).toBe('light');
    expect(resolveThemePreference('system', 'light', () => 'light')).toBe('system');
  });

  it('falls back to localStorage when the server value is missing or invalid', () => {
    expect(resolveThemePreference(undefined, 'light', () => null)).toBe('light');
    expect(resolveThemePreference('', 'light', () => null)).toBe('light');
    expect(resolveThemePreference('bogus', 'dark', () => null)).toBe('dark');
  });

  it('falls back to the data-theme attribute when server and localStorage miss', () => {
    expect(resolveThemePreference(undefined, null, () => 'light')).toBe('light');
    expect(resolveThemePreference(undefined, '', () => 'dark')).toBe('dark');
  });

  it('hard-defaults to following the OS only as a last resort', () => {
    expect(resolveThemePreference(undefined, null, () => null)).toBe('system');
    expect(resolveThemePreference(undefined, null, () => 'bogus')).toBe('system');
    expect(DEFAULT_THEME).toBe('system');
  });

  it('reads the attribute lazily, never when an earlier source already answers', () => {
    const getAttr = vi.fn(() => 'light');
    resolveThemePreference('dark', null, getAttr);
    resolveThemePreference(undefined, 'system', getAttr);
    expect(getAttr).not.toHaveBeenCalled();
  });

  it('a missing server value never clobbers a present localStorage value (regression)', () => {
    // The systemic dark-flash bug: the active device had no server-scoped
    // theme, so `prefs['theme'] || 'dark'` returned 'dark' and overwrote the
    // light value the FOUC script had already applied from localStorage.
    expect(resolveThemePreference(undefined, 'light', () => 'light')).toBe('light');
  });
});

describe('resolving a theme against the OS', () => {
  it('only `system` consults the OS', () => {
    expect(resolveTheme('light', false)).toBe('light');
    expect(resolveTheme('dark', true)).toBe('dark');
    expect(resolveTheme('system', true)).toBe('light');
    expect(resolveTheme('system', false)).toBe('dark');
  });

  it('every resolved theme has a background to paint before any stylesheet', () => {
    expect(THEME_BG.light).toBe('#ffffff');
    expect(THEME_BG.dark).toBe('#07172e');
  });
});

describe('font key resolution', () => {
  const ALL: FontFamily[] = [
    'monospace', 'system', 'inter', 'jetbrains-mono', 'ibm-plex-mono', 'fira-code',
  ];

  it('passes through every option the picker offers', () => {
    for (const font of ALL) expect(resolveFontKey(font)).toBe(font);
  });

  it('defaults anything unusable to Fira Code', () => {
    for (const stored of [null, undefined, '', 'comic-sans', 'MONOSPACE']) {
      expect(resolveFontKey(stored)).toBe('fira-code');
    }
    expect(DEFAULT_FONT_FAMILY).toBe('fira-code');
  });

  it('never accepts an INHERITED key as a font', () => {
    // The key comes out of localStorage, and `'toString' in FONT_FAMILY_VALUES`
    // is true. Accepting it would write `Object.prototype.toString`'s source
    // text into --font-ui, and read a function out of the ligature map so
    // `features.text` came back undefined.
    for (const stored of ['toString', 'constructor', 'valueOf', '__proto__', 'hasOwnProperty']) {
      expect(resolveFontKey(stored)).toBe('fira-code');
    }
  });

  it('resolves a real pair for every key it can return', () => {
    // The two halves of the same guard: whatever `resolveFontKey` hands back
    // must index BOTH maps to real values, since the caller reads both with it.
    for (const stored of ['toString', 'comic-sans', null, 'inter']) {
      const key = resolveFontKey(stored);
      expect(typeof FONT_FAMILY_VALUES[key]).toBe('string');
      expect(typeof fontFeaturesFor(key).text).toBe('string');
    }
  });

  it('never resolves to a key the family map lacks', () => {
    // The load-bearing half: the caller reads BOTH maps with this key, so a key
    // outside the map would take a stack from one and `normal` features from
    // the other, and `normal` is not "ligatures off".
    for (const stored of [null, 'nonsense', 'fira-code', 'inter']) {
      expect(FONT_FAMILY_VALUES[resolveFontKey(stored)]).toBeTruthy();
    }
  });

  it("keeps Fira Code's fallback chain on the system mono, never bare monospace", () => {
    const stack = FONT_FAMILY_VALUES['fira-code'];
    expect(stack.startsWith("'Fira Code', ui-monospace,")).toBe(true);
    expect(stack).not.toBe("'Fira Code', monospace");
  });
});

describe('ligature features', () => {
  it('are OFF for text and ON for code, with explicit zeros, for Fira Code only', () => {
    expect(fontFeaturesFor('fira-code')).toEqual({
      text: '"liga" 0, "calt" 0',
      code: '"liga" 1, "calt" 1',
    });
  });

  it('resolve BOTH to normal for every other font', () => {
    for (const font of ['monospace', 'system', 'inter', 'jetbrains-mono', 'ibm-plex-mono'] as FontFamily[]) {
      expect(fontFeaturesFor(font)).toEqual(FONT_FEATURES_DEFAULT);
    }
  });

  it('never spells the OFF value `normal`', () => {
    // `liga` and `calt` are default-ON, so `normal` renders identically to `1`
    // and the whole feature would be inert. This is the assertion that caught a
    // shipped no-op once already.
    expect(fontFeaturesFor('fira-code').text).not.toBe('normal');
    expect(fontFeaturesFor('fira-code').text).toMatch(/"liga"\s+0/);
  });
});

describe('ui scale', () => {
  it('snaps to the 12.5 grid and holds inside the bounds', () => {
    expect(clampUiScale(100)).toBe(100);
    expect(clampUiScale(137.5)).toBe(137.5);
    expect(clampUiScale(115)).toBe(112.5);
    expect(clampUiScale(50)).toBe(75);
    expect(clampUiScale(300)).toBe(200);
  });

  it('reads the pre-grid enum values old devices still carry', () => {
    expect(parseUiScale('small')).toBe(100);
    expect(parseUiScale('medium')).toBe(112.5);
    expect(parseUiScale('large')).toBe(125);
  });

  it('parses a stored number, snapping it', () => {
    expect(parseUiScale('125')).toBe(125);
    expect(parseUiScale('112.5')).toBe(112.5);
    expect(parseUiScale('115')).toBe(112.5);
  });

  it('answers null for nothing usable, rather than the default', () => {
    // `null` is what leaves --user-ui-scale UNSET so the stylesheet's own
    // fallback answers. Writing the default inline looks identical and then
    // quietly beats any later override of that property.
    // `toString` is in the list because the legacy map is indexed by a stored
    // string, so an inherited key must not read as a legacy scale.
    for (const raw of [null, undefined, '', 'huge', 'toString', 'constructor']) {
      expect(parseUiScale(raw)).toBeNull();
    }
    expect(UI_SCALE_DEFAULT).toBe(100);
  });
});

describe('reduced motion', () => {
  it('follows the OS only under `system`', () => {
    // The whole contract: 3 values x OS on/off.
    expect(resolveReducedMotion('system', true)).toBe(true);
    expect(resolveReducedMotion('system', false)).toBe(false);
    expect(resolveReducedMotion('reduce', true)).toBe(true);
    expect(resolveReducedMotion('reduce', false)).toBe(true);
    expect(resolveReducedMotion('full', true)).toBe(false);
    expect(resolveReducedMotion('full', false)).toBe(false);
  });

  it('reads anything unknown as the default, which follows the OS', () => {
    expect(DEFAULT_MOTION).toBe('system');
    expect(MOTION_PREFS).toEqual(['system', 'reduce', 'full']);
    expect(parseMotion('reduce')).toBe('reduce');
    expect(parseMotion(null)).toBe('system');
    expect(parseMotion('')).toBe('system');
    expect(parseMotion('toString')).toBe('system');
    expect(parseMotion('REDUCE')).toBe('system');
  });
});

describe('the animation duration scale', () => {
  it('is 1 at the slider centre and the reciprocal of the speed elsewhere', () => {
    expect(durationScaleFor(0, false)).toBe(1);
    expect(durationScaleFor(10, false)).toBeCloseTo(0.1, 10);
    expect(durationScaleFor(-10, false)).toBeCloseTo(10, 10);
  });

  it('collapses under reduced motion whatever the slider says', () => {
    for (const pos of [-10, 0, 10]) {
      expect(durationScaleFor(pos, true)).toBe(REDUCED_MOTION_DURATION_SCALE);
    }
  });

  it('stays above zero, so a transition still ends and still fires its end event', () => {
    // A 0s transition never starts, so it fires no `transitionend`. Code
    // that waits on one would then hang on its fallback timer.
    expect(REDUCED_MOTION_DURATION_SCALE).toBeGreaterThan(0);
    // And short enough that the slowest token (0.5s) ends inside one frame.
    expect(500 * REDUCED_MOTION_DURATION_SCALE).toBeLessThan(16);
  });

  it('parses a stored slider position, clamping and defaulting', () => {
    expect(parseAnimationSpeed('3')).toBe(3);
    expect(parseAnimationSpeed('-4')).toBe(-4);
    expect(parseAnimationSpeed('99')).toBe(10);
    expect(parseAnimationSpeed('-99')).toBe(-10);
    expect(parseAnimationSpeed(null)).toBe(0);
    expect(parseAnimationSpeed('junk')).toBe(0);
  });
});
