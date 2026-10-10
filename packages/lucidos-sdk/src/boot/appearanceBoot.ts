/**
 * The appearance FOUC script: one program, embedded in two documents.
 *
 * It resolves the device's appearance from localStorage: theme mode, theme,
 * font, ligature settings, UI scale, motion, theme effects and style overrides.
 * It writes them onto `<html>` **before any module loads**, so the first frame
 * is already the user's appearance, not a default corrected a moment later.
 *
 * **Why it is bundled rather than imported.** Both embed sites need
 * parser-blocking JavaScript with no imports and no network round trip:
 *
 *   - the app shell, inlined into `index.html`'s `<head>` by the
 *     `lucidos-appearance-boot` Vite plugin;
 *   - every app iframe, served by the engine as `/api/v1/sdk-prefs.js`
 *     (`api/sdk_prefs.rs` `include_str!`s the bundle).
 *
 * That is a real constraint at RUNTIME and it used to be taken as a constraint
 * on the SOURCE too, so the two documents carried two hand-copied programs plus
 * two more copies of the same values in the store and the SDK. They drifted by
 * construction and were held together by source-scanning guards. esbuild
 * removes the premise: one source, two dependency-free IIFEs, built by
 * `npm run build` into `../generated/` and checked in (see the staleness test,
 * and `api/sdk_prefs.rs` for why a committed artifact rather than a build-time
 * dependency).
 *
 * Keep this file free of anything the two documents do not share. The shell's
 * boot-splash gradient and its theme telemetry live in `host.ts`, because they
 * are the shell's, not the contract's.
 */
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
  /**
   * Honour `?style-reset` by clearing the stored overrides before applying
   * them. The shell's escape hatch out of a value that made the UI unusable.
   *
   * Off for iframes on purpose: the shell removes the key before an iframe
   * loads, so there is nothing left for that realm to clear, and an app URL
   * that happened to carry the parameter should not wipe the user's map.
   */
  styleReset: boolean;
  /**
   * Publish `--duration-scale` from the Animation speed slider and reduced
   * motion. Shell only: an app frame's stylesheet pins the scale to 1, and an
   * inline value here would beat it.
   */
  durationScale: boolean;
}

export interface BootResult {
  /** The raw stored value, for the shell's telemetry. */
  raw: string | null;
  mode: ThemeMode;
  resolved: ResolvedThemeMode;
  prefersLight: boolean;
  reducedMotion: boolean;
}

/**
 * Resolve and apply every appearance value. Returns what it resolved, so the
 * shell can log it without reading the DOM back.
 *
 * Storage goes through `_storage.ts`, which is what makes the shell and its
 * iframes read the SAME per-workspace keys. It already derives the slug both
 * ways this script needs (from the shell's stamped `<base href="/<slug>/">`,
 * and from the path before `/app/` for an iframe that has no `<base>`), so
 * there is no fourth copy of that derivation here. The `no-raw-storage` guard
 * enforces it.
 */
/**
 * The values the engine resolved for this device and prepended to this script,
 * or null in a document it did not seed.
 *
 * An isolated app frame is not same-origin with the shell, so it inherits none
 * of the mirror writes `wsLocalGet` reads. This script is parser-blocking and
 * settles first, so nothing async can feed it. That is why the values arrive in
 * its own body rather than over a channel. See `api/sdk_prefs.rs`.
 *
 * The shell seeds nothing and falls through to storage, unchanged. The SDK's
 * `autocorrectStamp.ts` reads the same seed, through this function.
 */
export function servedPrefs(): Record<string, string> | null {
  // `globalThis`, not `window`. The engine writes `window.__lucidosPrefs`, and
  // in a browser the two are one object. This form also runs where there is no
  // `window` at all.
  const served = (globalThis as { __lucidosPrefs?: unknown }).__lucidosPrefs;
  return served && typeof served === 'object' ? served as Record<string, string> : null;
}

/**
 * The engine's value for `serverKey`, else the stored one.
 *
 * Server first is the precedence `appearance.ts` documents for the live
 * re-apply. So first paint and every repaint after it rank the two sources the
 * same way.
 */
function seeded(served: Record<string, string> | null, serverKey: string, storageKey: string):
  string | null {
  const value = served?.[serverKey];
  return typeof value === 'string' && value !== '' ? value : wsLocalGet(storageKey);
}

export function applyAppearanceBoot(opts: BootOptions): BootResult {
  const d = document.documentElement;
  const served = servedPrefs();

  // Theme mode. Nothing saved means follow the OS.
  const raw = seeded(served, THEME_MODE_KEY, THEME_MODE_STORAGE_KEY);
  const mode = raw && (THEME_MODES as readonly string[]).includes(raw)
    ? raw as ThemeMode
    : DEFAULT_THEME_MODE;
  const prefersLight = matchMedia('(prefers-color-scheme: light)').matches;
  const resolved = resolveThemeMode(mode, prefersLight);
  d.setAttribute(THEME_MODE_ATTRIBUTE, resolved);
  // `?style-reset` is the way out of an unreadable theme as well as of the
  // overrides, so it drops the theme's cache too.
  const styleReset = opts.styleReset && styleResetRequested(location.search);
  if (styleReset) wsLocalRemove(THEME_STORAGE_KEY);
  // The theme, for this mode's map and its fonts. A corrupt seed parses to the
  // empty theme.
  const theme = parseResolvedTheme(seeded(served, THEME_SEED_KEY, THEME_STORAGE_KEY));
  const themeTokens = theme[resolved];
  const bg = themeBackground(themeTokens) ?? THEME_MODE_BG[resolved];
  d.style.setProperty('--bg-primary', bg);
  // Inline `background` as well as the custom property: it covers the iOS
  // WKWebView white flash on a PWA cold restart, before any stylesheet has
  // applied its own `html { background: var(--bg-primary) }` rule.
  d.style.background = bg;

  // Font: the user's pick, else the theme's, else the fallback. The key is
  // resolved ONCE and both maps are then read with it, which is what keeps the
  // family and its ligature settings from disagreeing. A workspace font paints
  // only with an entry at hand: the picked one's, and the theme's own.
  const picked = parseWorkspaceFont(
    seeded(served, WORKSPACE_FONT_SEED_KEY, WORKSPACE_FONT_STORAGE_KEY),
  );
  const known = picked ? [picked, ...theme.workspace_fonts] : theme.workspace_fonts;
  const font = resolveFont(seeded(served, PREF_FONT_FAMILY.key, 'lucidos-font-family'), theme.fonts, known);
  d.style.setProperty('--font-ui', font.stack);
  d.style.setProperty('--font-features-text', font.features.text);
  d.style.setProperty('--font-features-code', font.features.code);
  d.setAttribute(FONT_BOLD_ATTRIBUTE, fontBoldMark(font));
  // The theme's code font rides in its tokens; its faces register here too.
  registerFontsInUse(font, known, theme.fonts.mono, dataMountUrl);

  // Scale. Snapped to the grid here, so a pre-grid saved value like "115" does
  // not paint at 115% for one frame before the app boots, re-clamps to 112.5%
  // and re-paints. Left UNSET when nothing is stored, so the stylesheet's own
  // fallback answers rather than an inline value that would beat an override.
  // `text-size` and `font-size` are the pre-grid aliases, read in the order
  // `ui.applyPreferences` reads them so the two cannot resolve differently.
  const scale = parseUiScale(
    served?.[PREF_UI_SCALE.key] || served?.['text-size'] || served?.['font-size']
    || wsLocalGet('lucidos-ui-scale'),
  );
  if (scale !== null) d.style.setProperty('--user-ui-scale', `${scale}%`);

  // Motion. Resolved here, before the boot splash markup parses, so a device
  // that asked for calm never sees the splash move for a frame.
  const reducedMotion = resolveReducedMotion(
    parseMotion(seeded(served, PREF_MOTION.key, MOTION_STORAGE_KEY)),
    matchMedia(REDUCED_MOTION_QUERY).matches,
  );
  d.setAttribute('data-motion', motionAttribute(reducedMotion));
  if (opts.durationScale) {
    const position = parseAnimationSpeed(wsLocalGet(ANIMATION_SPEED_STORAGE_KEY));
    d.style.setProperty('--duration-scale', String(durationScaleFor(position, reducedMotion)));
  }

  // Theme effects, before first paint, so a device on `reduce` never shows a
  // theme's glow for a frame. CSS drops the part shadows and filters.
  const reducedThemeEffects = resolveReducedThemeEffects(
    parseThemeEffects(seeded(served, PREF_THEME_EFFECTS.key, THEME_EFFECTS_STORAGE_KEY)),
    matchMedia(REDUCED_TRANSPARENCY_QUERY).matches,
    matchMedia(MORE_CONTRAST_QUERY).matches,
  );
  d.setAttribute('data-theme-effects', themeEffectsAttribute(reducedThemeEffects));

  // The theme sits between the stylesheet and the style remote, so an override
  // still wins over it.
  for (const name of Object.keys(themeTokens)) {
    d.style.setProperty(name, themeTokens[name]);
  }
  // A background that is not a hex literal reaches the canvas through the var.
  if (themeTokens['--bg-primary'] && !themeBackground(themeTokens)) d.style.background = 'var(--bg-primary)';

  // The live style remote's first-paint seed. LAST on purpose: everything above
  // writes properties the remote is allowed to override, and inline properties
  // are last-write-wins.
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
    /* a corrupt map must never break FOUC */
  }

  return { raw, mode, resolved, prefersLight, reducedMotion };
}
