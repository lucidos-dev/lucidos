import {
  PREF_FONT_FAMILY, PREF_MOTION, PREF_THEME_EFFECTS, PREF_UI_SCALE,
} from '../generated/preference-catalog';
import {
  ANIMATION_SPEED_STORAGE_KEY,
  THEME_EFFECTS_STORAGE_KEY,
  THEME_SEED_KEY,
  THEME_STORAGE_KEY,
  MORE_CONTRAST_QUERY,
  MOTION_STORAGE_KEY,
  REDUCED_TRANSPARENCY_QUERY,
  REDUCED_MOTION_QUERY,
  STYLE_OVERRIDES_STORAGE_KEY,
  THEME_MODE_ATTRIBUTE,
  THEME_MODE_KEY,
  THEME_MODE_STORAGE_KEY,
  THEME_MODES,
  THEME_MODE_BG,
  DEFAULT_THEME_MODE,
  durationScaleFor,
  themeBackground,
  FONT_BOLD_ATTRIBUTE,
  fontBoldMark,
  themeEffectsAttribute,
  parseThemeEffects,
  parseResolvedTheme,
  resolveReducedThemeEffects,
  motionAttribute,
  parseAnimationSpeed,
  parseMotion,
  parseStyleOverrides,
  parseUiScale,
  parseWorkspaceFont,
  resolveFont,
  resolveReducedMotion,
  resolveThemeMode,
  styleResetRequested,
  WORKSPACE_FONT_SEED_KEY,
  WORKSPACE_FONT_STORAGE_KEY,
  type ResolvedThemeMode,
  type ThemeMode,
} from '../appearance';
import { dataMountUrl } from '../_fetch';
import { wsLocalGet, wsLocalRemove } from '../_storage';
import { registerFontsInUse } from '../fontFaces';

export interface BootOptions {
  styleReset: boolean;
  durationScale: boolean;
}

export interface BootResult {
  raw: string | null;
  mode: ThemeMode;
  resolved: ResolvedThemeMode;
  prefersLight: boolean;
  reducedMotion: boolean;
}

export function servedPrefs(): Record<string, string> | null {
  const served = (globalThis as { __lucidosPrefs?: unknown }).__lucidosPrefs;
  return served && typeof served === 'object' ? served as Record<string, string> : null;
}

function seeded(served: Record<string, string> | null, serverKey: string, storageKey: string):
  string | null {
  const value = served?.[serverKey];
  return typeof value === 'string' && value !== '' ? value : wsLocalGet(storageKey);
}

export function applyAppearanceBoot(opts: BootOptions): BootResult {
  const d = document.documentElement;
  const served = servedPrefs();

  const raw = seeded(served, THEME_MODE_KEY, THEME_MODE_STORAGE_KEY);
  const mode = raw && (THEME_MODES as readonly string[]).includes(raw)
    ? raw as ThemeMode
    : DEFAULT_THEME_MODE;
  const prefersLight = matchMedia('(prefers-color-scheme: light)').matches;
  const resolved = resolveThemeMode(mode, prefersLight);
  d.setAttribute(THEME_MODE_ATTRIBUTE, resolved);
  const styleReset = opts.styleReset && styleResetRequested(location.search);
  if (styleReset) wsLocalRemove(THEME_STORAGE_KEY);
  const theme = parseResolvedTheme(seeded(served, THEME_SEED_KEY, THEME_STORAGE_KEY));
  const themeTokens = theme[resolved];
  const bg = themeBackground(themeTokens) ?? THEME_MODE_BG[resolved];
  d.style.setProperty('--bg-primary', bg);
  d.style.background = bg;

  const picked = parseWorkspaceFont(
    seeded(served, WORKSPACE_FONT_SEED_KEY, WORKSPACE_FONT_STORAGE_KEY),
  );
  const known = picked ? [picked, ...theme.workspace_fonts] : theme.workspace_fonts;
  const font = resolveFont(seeded(served, PREF_FONT_FAMILY.key, 'lucidos-font-family'), theme.fonts, known);
  d.style.setProperty('--font-ui', font.stack);
  d.style.setProperty('--font-features-text', font.features.text);
  d.style.setProperty('--font-features-code', font.features.code);
  d.setAttribute(FONT_BOLD_ATTRIBUTE, fontBoldMark(font));
  registerFontsInUse(font, known, theme.fonts.mono, dataMountUrl);

  const scale = parseUiScale(
    served?.[PREF_UI_SCALE.key] || served?.['text-size'] || served?.['font-size']
    || wsLocalGet('lucidos-ui-scale'),
  );
  if (scale !== null) d.style.setProperty('--user-ui-scale', `${scale}%`);

  const reducedMotion = resolveReducedMotion(
    parseMotion(seeded(served, PREF_MOTION.key, MOTION_STORAGE_KEY)),
    matchMedia(REDUCED_MOTION_QUERY).matches,
  );
  d.setAttribute('data-motion', motionAttribute(reducedMotion));
  if (opts.durationScale) {
    const position = parseAnimationSpeed(wsLocalGet(ANIMATION_SPEED_STORAGE_KEY));
    d.style.setProperty('--duration-scale', String(durationScaleFor(position, reducedMotion)));
  }

  const reducedThemeEffects = resolveReducedThemeEffects(
    parseThemeEffects(seeded(served, PREF_THEME_EFFECTS.key, THEME_EFFECTS_STORAGE_KEY)),
    matchMedia(REDUCED_TRANSPARENCY_QUERY).matches,
    matchMedia(MORE_CONTRAST_QUERY).matches,
  );
  d.setAttribute('data-theme-effects', themeEffectsAttribute(reducedThemeEffects));

  for (const name of Object.keys(themeTokens)) {
    d.style.setProperty(name, themeTokens[name]);
  }
  if (themeTokens['--bg-primary'] && !themeBackground(themeTokens)) d.style.background = 'var(--bg-primary)';

  try {
    if (styleReset) {
      wsLocalRemove(STYLE_OVERRIDES_STORAGE_KEY);
    } else {
      const overrides = parseStyleOverrides(
        seeded(served, 'style_overrides', STYLE_OVERRIDES_STORAGE_KEY),
      );
      for (const name of Object.keys(overrides)) {
        d.style.setProperty(name, overrides[name]);
      }
    }
  } catch {
  }

  return { raw, mode, resolved, prefersLight, reducedMotion };
}
