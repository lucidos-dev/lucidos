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
/**
 * The values the engine resolved for this device and prepended to this script,
 * or null in a document it did not seed.
 *
 * An isolated app frame is not same-origin with the shell, so it inherits none
 * of the mirror writes `wsLocalGet` reads. This script is parser-blocking and
 * settles first, so nothing async can feed it. That is why the values arrive in
 * its own body rather than over a channel. See `api/sdk_prefs.rs`.
 *
 * The shell seeds nothing and falls through to storage, unchanged.
 */
function servedPrefs(): Record<string, string> | null {
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

  // Theme. Nothing saved means follow the OS.
  const raw = seeded(served, 'theme', 'lucidos-theme');
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
  const fontKey = resolveFontKey(seeded(served, 'font-family', 'lucidos-font-family'));
  d.style.setProperty('--font-ui', FONT_FAMILY_VALUES[fontKey]);
  const features = fontFeaturesFor(fontKey);
  d.style.setProperty('--font-features-text', features.text);
  d.style.setProperty('--font-features-code', features.code);

  // Scale. Snapped to the grid here, so a pre-grid saved value like "115" does
  // not paint at 115% for one frame before the app boots, re-clamps to 112.5%
  // and re-paints. Left UNSET when nothing is stored, so the stylesheet's own
  // fallback answers rather than an inline value that would beat an override.
  // `text-size` and `font-size` are the pre-grid aliases, read in the order
  // `ui.applyPreferences` reads them so the two cannot resolve differently.
  const scale = parseUiScale(
    served?.['ui-scale'] || served?.['text-size'] || served?.['font-size']
    || wsLocalGet('lucidos-ui-scale'),
  );
  if (scale !== null) d.style.setProperty('--user-ui-scale', `${scale}%`);

  // The live style remote's first-paint seed. LAST on purpose: everything above
  // writes properties the remote is allowed to override, and inline properties
  // are last-write-wins.
  try {
    if (opts.styleReset && styleResetRequested(location.search)) {
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

  return { raw, theme, resolved, prefersLight };
}
