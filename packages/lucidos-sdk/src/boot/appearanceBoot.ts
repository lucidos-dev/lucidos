/**
 * The appearance FOUC script: one program, embedded in two documents.
 *
 * It resolves the device's theme, font, ligature settings, UI scale and style
 * overrides from localStorage and writes them onto `<html>` **before any module
 * loads**, so the first frame is already the user's appearance instead of a
 * default that gets corrected a moment later.
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
 * and `api/sdk_fonts.rs` for why a committed artifact rather than a build-time
 * dependency).
 *
 * Keep this file free of anything the two documents do not share. The shell's
 * boot-splash gradient and its theme telemetry live in `host.ts`, because they
 * are the shell's, not the contract's.
 */
import {
  FONT_FAMILY_VALUES,
  STYLE_OVERRIDES_STORAGE_KEY,
  THEMES,
  THEME_BG,
  DEFAULT_THEME,
  fontFeaturesFor,
  parseStyleOverrides,
  parseUiScale,
  resolveFontKey,
  resolveTheme,
  styleResetRequested,
  type ResolvedTheme,
  type ThemePref,
} from '../appearance';
import { wsLocalGet, wsLocalRemove } from '../_storage';

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
}

export interface BootResult {
  /** The raw stored value, for the shell's telemetry. */
  raw: string | null;
  theme: ThemePref;
  resolved: ResolvedTheme;
  prefersLight: boolean;
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
export function applyAppearanceBoot(opts: BootOptions): BootResult {
  const d = document.documentElement;

  // Theme. Nothing saved means follow the OS.
  const raw = wsLocalGet('lucidos-theme');
  const theme = raw && (THEMES as readonly string[]).includes(raw)
    ? raw as ThemePref
    : DEFAULT_THEME;
  const prefersLight = matchMedia('(prefers-color-scheme: light)').matches;
  const resolved = resolveTheme(theme, prefersLight);
  d.setAttribute('data-theme', resolved);
  const bg = THEME_BG[resolved];
  d.style.setProperty('--bg-primary', bg);
  // Inline `background` as well as the custom property: it covers the iOS
  // WKWebView white flash on a PWA cold restart, before any stylesheet has
  // applied its own `html { background: var(--bg-primary) }` rule.
  d.style.background = bg;

  // Font. The key is resolved ONCE and both maps are then read with it, which
  // is what keeps the family and its ligature settings from disagreeing.
  const fontKey = resolveFontKey(wsLocalGet('lucidos-font-family'));
  d.style.setProperty('--font-ui', FONT_FAMILY_VALUES[fontKey]);
  const features = fontFeaturesFor(fontKey);
  d.style.setProperty('--font-features-text', features.text);
  d.style.setProperty('--font-features-code', features.code);

  // Scale. Snapped to the grid here, so a pre-grid saved value like "115" does
  // not paint at 115% for one frame before the app boots, re-clamps to 112.5%
  // and re-paints. Left UNSET when nothing is stored, so the stylesheet's own
  // fallback answers rather than an inline value that would beat an override.
  const scale = parseUiScale(wsLocalGet('lucidos-ui-scale'));
  if (scale !== null) d.style.setProperty('--user-ui-scale', `${scale}%`);

  // The live style remote's first-paint seed. LAST on purpose: everything above
  // writes properties the remote is allowed to override, and inline properties
  // are last-write-wins.
  try {
    if (opts.styleReset && styleResetRequested(location.search)) {
      wsLocalRemove(STYLE_OVERRIDES_STORAGE_KEY);
    } else {
      const overrides = parseStyleOverrides(wsLocalGet(STYLE_OVERRIDES_STORAGE_KEY));
      for (const name of Object.keys(overrides)) {
        d.style.setProperty(name, overrides[name]);
      }
    }
  } catch {
    /* a corrupt map must never break FOUC */
  }

  return { raw, theme, resolved, prefersLight };
}
