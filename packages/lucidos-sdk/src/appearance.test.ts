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
  readAppearancePush,
  sanitizeAppearancePush,
  DEFAULT_FONT_PREFERENCE,
  DEFAULT_MOTION,
  DEFAULT_THEME_MODE,
  EMPTY_THEME,
  FALLBACK_FONT,
  FOLLOW_THEME,
  FONT_CATALOG,
  FONT_PREFERENCES,
  FONT_STACKS,
  FONT_FEATURES_DEFAULT,
  MOTION_PREFS,
  REDUCED_MOTION_DURATION_SCALE,
  THEME_MODE_BG,
  UI_SCALE_DEFAULT,
  clampUiScale,
  durationScaleFor,
  fontFeaturesFor,
  fontStackFor,
  fontBoldMark,
  weightReachesBold,
  workspaceFontHasBold,
  registeredFaceWeight,
  parseAnimationSpeed,
  parseThemeEffects,
  parseMotion,
  parseUiScale,
  resolveReducedThemeEffects,
  resolveReducedMotion,
  resolveFont,
  resolveFontKey,
  resolveThemeMode,
  resolveThemeModePreference,
  type FontId,
  type ThemeFonts,
  type WorkspaceFont,
} from './appearance';

describe('theme preference precedence', () => {
  it('prefers a valid server value over everything else', () => {
    expect(resolveThemeModePreference('light', 'dark', () => 'dark')).toBe('light');
    expect(resolveThemeModePreference('system', 'light', () => 'light')).toBe('system');
  });

  it('falls back to localStorage when the server value is missing or invalid', () => {
    expect(resolveThemeModePreference(undefined, 'light', () => null)).toBe('light');
    expect(resolveThemeModePreference('', 'light', () => null)).toBe('light');
    expect(resolveThemeModePreference('bogus', 'dark', () => null)).toBe('dark');
  });

  it('falls back to the data-theme-mode attribute when server and localStorage miss', () => {
    expect(resolveThemeModePreference(undefined, null, () => 'light')).toBe('light');
    expect(resolveThemeModePreference(undefined, '', () => 'dark')).toBe('dark');
  });

  it('hard-defaults to following the OS only as a last resort', () => {
    expect(resolveThemeModePreference(undefined, null, () => null)).toBe('system');
    expect(resolveThemeModePreference(undefined, null, () => 'bogus')).toBe('system');
    expect(DEFAULT_THEME_MODE).toBe('system');
  });

  it('reads the attribute lazily, never when an earlier source already answers', () => {
    const getAttr = vi.fn(() => 'light');
    resolveThemeModePreference('dark', null, getAttr);
    resolveThemeModePreference(undefined, 'system', getAttr);
    expect(getAttr).not.toHaveBeenCalled();
  });

  it('a missing server value never clobbers a present localStorage value (regression)', () => {
    // The systemic dark-flash bug: the active device had no server-scoped
    // theme, so `prefs['theme-mode'] || 'dark'` returned 'dark' and overwrote the
    // light value the FOUC script had already applied from localStorage.
    expect(resolveThemeModePreference(undefined, 'light', () => 'light')).toBe('light');
  });
});

describe('resolving a theme against the OS', () => {
  it('only `system` consults the OS', () => {
    expect(resolveThemeMode('light', false)).toBe('light');
    expect(resolveThemeMode('dark', true)).toBe('dark');
    expect(resolveThemeMode('system', true)).toBe('light');
    expect(resolveThemeMode('system', false)).toBe('dark');
  });

  it('every resolved theme has a background to paint before any stylesheet', () => {
    expect(THEME_MODE_BG.light).toBe('#ffffff');
    expect(THEME_MODE_BG.dark).toBe('#07172e');
  });
});

describe('font key resolution', () => {
  const ALL: FontId[] = FONT_CATALOG.map(font => font.id);
  const NO_THEME_FONT: ThemeFonts = {};
  const THEME_GEIST: ThemeFonts = { ui: 'geist' };

  it('passes through every font in the catalog', () => {
    for (const font of ALL) expect(resolveFontKey(font, NO_THEME_FONT)).toBe(font);
  });

  it('an explicit pick wins over the theme', () => {
    for (const font of ALL) expect(resolveFontKey(font, THEME_GEIST)).toBe(font);
  });

  it('follow-the-theme paints the font the theme suggests', () => {
    expect(resolveFontKey(FOLLOW_THEME, THEME_GEIST)).toBe('geist');
    expect(resolveFontKey(FOLLOW_THEME, { ui: 'source-serif-4' })).toBe('source-serif-4');
  });

  it('an unset or unusable value follows the theme too', () => {
    for (const stored of [null, undefined, '', 'comic-sans', 'MONOSPACE']) {
      expect(resolveFontKey(stored, THEME_GEIST)).toBe('geist');
    }
  });

  it('a theme with no font falls back to Fira Code', () => {
    for (const stored of [FOLLOW_THEME, null, undefined, '', 'comic-sans']) {
      expect(resolveFontKey(stored, NO_THEME_FONT)).toBe('fira-code');
      expect(resolveFontKey(stored, EMPTY_THEME.fonts)).toBe('fira-code');
    }
    expect(FALLBACK_FONT).toBe('fira-code');
  });

  it('the preference defaults to following the theme', () => {
    expect(DEFAULT_FONT_PREFERENCE).toBe(FOLLOW_THEME);
    expect(FONT_PREFERENCES[0]).toBe(FOLLOW_THEME);
    expect(FONT_PREFERENCES.slice(1)).toEqual(ALL);
  });

  it('never accepts an INHERITED key as a font', () => {
    // The key comes out of localStorage, and `'toString' in FONT_STACKS` is
    // true. Accepting it would write `Object.prototype.toString`'s source text
    // into --font-ui.
    for (const stored of ['toString', 'constructor', 'valueOf', '__proto__', 'hasOwnProperty']) {
      expect(resolveFontKey(stored, NO_THEME_FONT)).toBe('fira-code');
    }
  });

  it('resolves a real pair for every key it can return', () => {
    // The caller reads BOTH maps with this key. A key outside the map would
    // take a stack from one and `normal` features from the other, and
    // `normal` is not "ligatures off".
    for (const stored of ['toString', 'comic-sans', null, 'inter', FOLLOW_THEME]) {
      for (const theme of [NO_THEME_FONT, THEME_GEIST]) {
        const key = resolveFontKey(stored, theme);
        expect(typeof fontStackFor(key)).toBe('string');
        expect(typeof fontFeaturesFor(key).text).toBe('string');
      }
    }
  });

  it("keeps Fira Code's fallback chain on the system mono, never bare monospace", () => {
    const stack = FONT_STACKS['fira-code'];
    expect(stack.startsWith("'Fira Code', ui-monospace,")).toBe(true);
    expect(stack).not.toBe("'Fira Code', monospace");
  });
});

describe('the bold mark', () => {
  const workspaceFont = (weights: string[]): WorkspaceFont => ({
    id: 'ws-pixel' as WorkspaceFont['id'],
    label: 'Pixel',
    family: 'ws-pixel' as WorkspaceFont['family'],
    stack: "'ws-pixel', monospace",
    group: 'mono',
    ligatures: false,
    faces: weights.map((weight, i) => ({ path: `pixel-${i}.woff2`, weight, style: 'normal' as const })),
  });
  const resolvedWorkspace = (font: WorkspaceFont) => resolveFont(font.id, {}, [font]);

  it('says `none` for a catalog UI font with no bold face', () => {
    expect(fontBoldMark(resolveFont('vt323', {}))).toBe('none');
  });

  it('says `face` for every catalog font with a bold face', () => {
    for (const font of FONT_CATALOG.filter(f => f.bold)) {
      expect(fontBoldMark(resolveFont(font.id, {})), font.id).toBe('face');
    }
  });

  it('reads a workspace font by its own faces', () => {
    expect(fontBoldMark(resolvedWorkspace(workspaceFont(['400'])))).toBe('none');
    expect(fontBoldMark(resolvedWorkspace(workspaceFont(['400', '700'])))).toBe('face');
    expect(fontBoldMark(resolvedWorkspace(workspaceFont(['100 900'])))).toBe('face');
  });

  it('does not count a bold italic as upright bold', () => {
    const font = workspaceFont(['400', '700']);
    font.faces[1].style = 'italic';
    expect(fontBoldMark(resolvedWorkspace(font))).toBe('none');
    expect(workspaceFontHasBold(font, 'italic')).toBe(true);
  });

  it('stretches only the heaviest face of a style with no bold up to 900', () => {
    const font = workspaceFont(['300', '400']);
    expect(font.faces.map(face => registeredFaceWeight(font, face))).toEqual(['300', '400 900']);
    const single = workspaceFont(['400']);
    expect(registeredFaceWeight(single, single.faces[0])).toBe('400 900');
    const bold = workspaceFont(['400', '700']);
    expect(bold.faces.map(face => registeredFaceWeight(bold, face))).toEqual(['400', '700']);
  });

  it('counts a face as bold from weight 600, at the heavy end of a range', () => {
    expect(weightReachesBold('600')).toBe(true);
    expect(weightReachesBold('300 500')).toBe(false);
    expect(weightReachesBold('300 700')).toBe(true);
  });
});

describe('ligature features', () => {
  it('are OFF for text and ON for code, with explicit zeros, for Fira Code only', () => {
    expect(fontFeaturesFor('fira-code')).toEqual({
      text: '"liga" 0, "calt" 0',
      code: '"liga" 1, "calt" 1',
    });
  });

  it('resolve BOTH to normal for every font without programming ligatures', () => {
    for (const font of FONT_CATALOG.filter(f => !f.ligatures)) {
      expect(fontFeaturesFor(font.id)).toEqual(FONT_FEATURES_DEFAULT);
    }
  });

  it('keep ligatures to code for every font that ships them', () => {
    for (const id of ['fira-code', 'jetbrains-mono', 'cascadia-code'] as const) {
      expect(fontFeaturesFor(id)).toEqual(fontFeaturesFor('fira-code'));
      expect(fontFeaturesFor(id).text).toMatch(/"liga"\s+0/);
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

describe('theme effects', () => {
  it('follows either OS signal under system, and an explicit choice over both', () => {
    expect(resolveReducedThemeEffects('system', false, false)).toBe(false);
    expect(resolveReducedThemeEffects('system', true, false)).toBe(true);
    expect(resolveReducedThemeEffects('system', false, true)).toBe(true);
    expect(resolveReducedThemeEffects('reduce', false, false)).toBe(true);
    expect(resolveReducedThemeEffects('full', true, true)).toBe(false);
  });

  it('parses only the three values, own keys only', () => {
    expect(parseThemeEffects('reduce')).toBe('reduce');
    expect(parseThemeEffects('full')).toBe('full');
    expect(parseThemeEffects(null)).toBe('system');
    expect(parseThemeEffects('toString')).toBe('system');
    expect(parseThemeEffects('REDUCE')).toBe('system');
  });
});

describe('the appearance push', () => {
  it('reads each value from the mirror the shell painted into', () => {
    const mirror: Record<string, string> = {
      'lucidos-theme-mode': 'light',
      'lucidos-theme-resolved': '{"light":{},"dark":{}}',
      'lucidos-ui-scale': '125',
      'lucidos-style-overrides': '{"--accent":"red"}',
    };
    expect(readAppearancePush(key => mirror[key] ?? null)).toEqual({
      'theme-mode': 'light',
      theme_resolved: '{"light":{},"dark":{}}',
      'ui-scale': '125',
      style_overrides: '{"--accent":"red"}',
    });
  });

  it('keeps only the known keys, and only strings', () => {
    expect(sanitizeAppearancePush({ 'ui-scale': '150', motion: 3, chat_model: 'x' }))
      .toEqual({ 'ui-scale': '150' });
    expect(sanitizeAppearancePush(null)).toBeNull();
    expect(sanitizeAppearancePush(['ui-scale'])).toBeNull();
  });
});
