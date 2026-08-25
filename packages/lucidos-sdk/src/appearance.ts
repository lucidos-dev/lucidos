/**
 * The appearance boot contract: the single source for what a device's theme, UI
 * font, ligature settings and UI scale resolve to.
 *
 * Four surfaces must agree on these values: the host store
 * (`store/actions/preferences.ts`), the host's inline FOUC script
 * (`index.html`), the app-iframe FOUC script (`api/sdk_prefs.rs`, served as
 * `/api/v1/sdk-prefs.js`), and the SDK's `ui.ts`. They paint at different
 * moments of one page load, so a disagreement between any two is a visible
 * flash. The two FOUC scripts are parser-blocking and cannot `import` at
 * runtime, which forces two self-contained programs. Both are built from
 * `boot/appearanceBoot.ts`, which reads this file like everyone else.
 *
 * **This module is pure**: no DOM, no storage, no network, so every rule is
 * unit-testable without a browser. Anything touching `document` belongs in
 * `boot/`. It is deliberately NOT re-exported from `index.ts`: nothing here
 * belongs on `window.lucidos`. That also keeps the whole SDK out of the host
 * store's module graph, which initialises a theme listener at import time.
 */

export type ThemePref = 'light' | 'dark' | 'system';
/** A theme with `system` already resolved against the OS. */
export type ResolvedTheme = 'light' | 'dark';

export type FontFamily =
  | 'monospace'
  | 'system'
  | 'inter'
  | 'jetbrains-mono'
  | 'ibm-plex-mono'
  | 'fira-code';

export const THEMES: readonly ThemePref[] = ['light', 'dark', 'system'];

/** What an unset `theme` preference means: follow the OS light/dark setting.
 *  A device that explicitly picked light or dark keeps its pick. */
export const DEFAULT_THEME: ThemePref = 'system';

/** The document background per resolved theme. Painted inline on `<html>` by
 *  every surface, so it is legible before any stylesheet has been parsed. */
export const THEME_BG: Record<ResolvedTheme, string> = {
  light: '#ffffff',
  dark: '#07172e',
};

/**
 * The CSS value each `font-family` option resolves to.
 *
 * Fira Code's chain is the FULL system-mono stack rather than a bare
 * `monospace`, because it is the default (ADR 0077). The tail is what paints
 * before the web font decodes, and on any device where it never loads, and bare
 * `monospace` is Courier. The other three keep short chains: a user who picked
 * one opted into the wait, and their natural fallback is not this stack.
 */
export const FONT_FAMILY_VALUES: Record<FontFamily, string> = {
  monospace: "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, 'Fira Code', 'JetBrains Mono', Monaco, Consolas, monospace",
  system: "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
  inter: "'Inter', system-ui, sans-serif",
  'jetbrains-mono': "'JetBrains Mono', monospace",
  'ibm-plex-mono': "'IBM Plex Mono', monospace",
  'fira-code': "'Fira Code', ui-monospace, SFMono-Regular, 'SF Mono', Menlo, 'JetBrains Mono', Monaco, Consolas, monospace",
};

/** What an unset `font-family` preference means. Served by the local engine
 *  (`api/sdk_fonts.rs`) rather than a CDN, so it needs no internet (ADR 0077). */
export const DEFAULT_FONT_FAMILY: FontFamily = 'fira-code';

/**
 * Opt-in web fonts, fetched from Google the first time one is selected.
 *
 * Fira Code is deliberately absent, and the asymmetry is the point: it is the
 * DEFAULT, so it must render offline and announce no boot to a third party.
 * It is vendored instead (ADR 0077), declared as an `@font-face` in
 * `styles/global/base.css` for the host and served from the engine to app
 * iframes.
 */
export const GOOGLE_FONT_URLS: Partial<Record<FontFamily, string>> = {
  inter: 'https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap',
  'jetbrains-mono': 'https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;600;700&display=swap',
  'ibm-plex-mono': 'https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600;700&display=swap',
};

/** The two values published as `--font-features-text` and
 *  `--font-features-code`. */
export interface FontFeaturePair {
  text: string;
  code: string;
}

/**
 * Programming ligatures belong to CODE, never to prose. The pair is published
 * as two custom properties, and the stylesheets decide where each applies
 * (`styles/global/base.css` and the engine's `api/sdk_iframe.css`).
 *
 * The OFF value is the explicit zeros and MUST NOT be spelled `normal`. `liga`
 * and `calt` are default-ON in CSS, so `normal` means the font's defaults and
 * renders identically to `1`. That leaves Fira Code's `calt` free to re-space a
 * typed `...` (tonsky/FiraCode#1561), and dropping the declaration disables
 * nothing.
 *
 * Every non-Fira font resolves BOTH to `normal`, which leaves its rendering
 * untouched: an unconditional `"liga" 0` would also kill the `fi` and `fl`
 * ligatures a proportional face like Inter wants.
 */
export const FONT_FEATURES_DEFAULT: FontFeaturePair = { text: 'normal', code: 'normal' };
const FONT_FEATURES: Partial<Record<FontFamily, FontFeaturePair>> = {
  'fira-code': { text: '"liga" 0, "calt" 0', code: '"liga" 1, "calt" 1' },
};

export const UI_SCALE_MIN = 75;
export const UI_SCALE_MAX = 200;
/** 12.5% keeps the root font-size on integer pixels (16 x 0.125 = 2px per
 *  step). Every `rem` then resolves to an integer, so 1px borders do not
 *  anti-alias at varying widths across the layout. */
export const UI_SCALE_STEP = 12.5;
export const UI_SCALE_DEFAULT = 100;

/** Pre-grid enum values, still present in old stored preferences. `medium`
 *  snaps to 112.5 on the 12.5 grid. */
export const LEGACY_UI_SCALES: Record<string, number> = {
  small: 100,
  medium: 112.5,
  large: 125,
};

/**
 * Which theme preference applies, given what each source knows. Precedence:
 *
 *   1. the server-provided value, when present and valid;
 *   2. else the `lucidos-theme` localStorage value the FOUC script read;
 *   3. else the `data-theme` attribute the FOUC script already applied;
 *   4. else {@link DEFAULT_THEME}, follow the OS.
 *
 * Load-bearing invariant: a MISSING server theme must NEVER clobber the value
 * the synchronous client resolver already settled on. A `prefs['theme'] ||
 * 'dark'` breaks it, flipping every app iframe to dark on a device that stored
 * only `ui-scale` while localStorage said light.
 *
 * `getAttr` is a thunk so the DOM is read only as a last resort, which keeps
 * the common path side-effect-free. Returns a raw preference, and the caller
 * resolves `system` via matchMedia.
 */
export function resolveThemePreference(
  server: string | undefined,
  local: string | null,
  getAttr: () => string | null,
): ThemePref {
  const valid = THEMES as readonly string[];
  if (server && valid.includes(server)) return server as ThemePref;
  if (local && valid.includes(local)) return local as ThemePref;
  const attr = getAttr();
  return attr === 'light' || attr === 'dark' ? attr : DEFAULT_THEME;
}

/** Collapse a preference to what actually paints. `system` asks the OS. */
export function resolveTheme(theme: ThemePref, prefersLight: boolean): ResolvedTheme {
  if (theme === 'system') return prefersLight ? 'light' : 'dark';
  return theme;
}

/**
 * How long the shell and every app iframe wait before sampling the OS
 * appearance, once something suggests it moved.
 *
 * Backgrounding an iOS app makes UIKit flip its trait collection to the
 * opposite appearance and straight back, to render both app-switcher snapshots
 * (rdar://7213631). WKWebView passes each flip into the page as a real
 * `prefers-color-scheme` change. The delay is long enough for the second half
 * of that pair to land, and short enough to read as immediate.
 *
 * A skew between the surfaces would only mean one repainting before another,
 * so this is here for the single definition rather than for agreement.
 */
export const SYSTEM_THEME_SETTLE_MS = 300;

/**
 * Which font a stored value selects, defaulting to {@link DEFAULT_FONT_FAMILY}.
 *
 * Every surface resolves the KEY once and then reads both maps with it, rather
 * than defaulting each lookup separately. That is not tidiness. A stored value
 * absent from the family map would take the default STACK while the feature
 * lookup fell through to `normal`. And `normal` means the font's defaults, so
 * Fira Code's ligatures would come back on for prose.
 */
export function resolveFontKey(stored: string | null | undefined): FontFamily {
  return stored && hasOwn(FONT_FAMILY_VALUES, stored)
    ? (stored as FontFamily)
    : DEFAULT_FONT_FAMILY;
}

/** The ligature pair for a font. Only fonts that ship programming ligatures
 *  get anything but `normal`. */
export function fontFeaturesFor(font: FontFamily): FontFeaturePair {
  return hasOwn(FONT_FEATURES, font) ? FONT_FEATURES[font]! : FONT_FEATURES_DEFAULT;
}

/**
 * An OWN key, never an inherited one.
 *
 * Every lookup here is keyed by a value out of localStorage, and `in` (or a
 * bare index) walks the prototype chain: `'toString' in FONT_FAMILY_VALUES` is
 * true, so a stored `toString` would resolve to a FONT KEY, and the caller
 * would then write `Object.prototype.toString`'s source text into `--font-ui`.
 * The same value read out of `FONT_FEATURES` is a function rather than a pair,
 * so `features.text` would be `undefined`.
 *
 * `hasOwnProperty` off the prototype rather than `Object.hasOwn`: this module
 * is bundled to es2015 for the boot script, and esbuild transforms syntax
 * without polyfilling built-ins. An ES2022 method would simply be missing on an
 * old WebView, in the one script that has nothing to fall back to.
 */
function hasOwn(obj: object, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(obj, key);
}

/** Snap to the 12.5 grid and hold inside the bounds. */
export function clampUiScale(scale: number): number {
  const snapped = Math.round(scale / UI_SCALE_STEP) * UI_SCALE_STEP;
  return Math.max(UI_SCALE_MIN, Math.min(UI_SCALE_MAX, snapped));
}

/**
 * A stored `ui-scale` string as a clamped percentage, or `null` when there is
 * nothing usable to apply.
 *
 * `null` rather than the default on purpose: the FOUC script must leave
 * `--user-ui-scale` UNSET when nothing is stored, so the stylesheet's own
 * `var(--user-ui-scale, 100%)` fallback answers. Writing 100% inline instead
 * would look identical and quietly beat any later override of that property.
 */
export function parseUiScale(raw: string | null | undefined): number | null {
  if (!raw) return null;
  const n = hasOwn(LEGACY_UI_SCALES, raw) ? LEGACY_UI_SCALES[raw] : parseFloat(raw);
  if (isNaN(n)) return null;
  return clampUiScale(n);
}

// --- The live style remote ---
//
// A `style_overrides` preference holds a JSON object of custom property name to
// value, applied straight onto `<html>`. Preferences fan out over the
// `PreferencesChanged` SSE, so a design value can be retuned on a running app
// with no rebuild and no Apply.
//
// That reach is why the validation below exists. Any app can write preferences
// through `lucidos.preferences.set`, and the chat agent can over the HTTP API.
// The map is therefore an UNTRUSTED input path into the host's own inline
// style, and everything reaching `setProperty` passes through here first.
//
// It lives in this contract because the boot script applies the same map before
// any module can run. The two must not reach different verdicts a moment apart.
// `utils/styleOverrides.ts` re-exports these.

/** Workspace-scoped localStorage key for the FOUC seed. Scoping is automatic in
 *  the app realm (`workspaceStorage.ts` overrides `Storage.prototype`); the boot
 *  script wraps it in `wsKey()` by hand. */
export const STYLE_OVERRIDES_STORAGE_KEY = 'lucidos-style-overrides';

/** URL parameter that clears the map before first paint. The escape hatch from
 *  a value that made the UI unusable, so it must not depend on any of the UI
 *  being legible. */
export const STYLE_RESET_PARAM = 'style-reset';

/** Cap on entries. A design remote tunes tens of values; a map in the thousands
 *  is a runaway writer, not a user. */
export const MAX_STYLE_OVERRIDES = 200;

/** Cap on one value's length. The longest real token in `base.css` is a layered
 *  box-shadow at 94 characters. */
export const MAX_STYLE_VALUE_LENGTH = 120;

/** Only a custom property can be set: never `color`, never a selector. */
const NAME_RE = /^--[a-z][a-z0-9-]*$/;

/**
 * Rejected value shapes, and why each one is here.
 *
 * | Shape | Why |
 * |---|---|
 * | `;` | closes the declaration, so the rest becomes a SECOND one |
 * | `{` `}` | closes the rule and opens a new selector block |
 * | `<` `>` | `</style>` breaks out of an inlined stylesheet context |
 * | `@` | `@import` pulls in a remote stylesheet |
 * | `\` | CSS escapes (`\3b`) spell any of the above past a naive scan |
 * | `url(` | requests an origin the value's author chose, leaking the page view |
 * | `image-set(` | the same hazard by another name |
 * | `expression(` | legacy IE dynamic properties, still parsed by some engines |
 * | `/*` | opens a comment that swallows the rest of the block |
 *
 * `var(`, `color-mix(`, `rgba(`, `calc(` and the gradient functions are all
 * fine and deliberately allowed: they are what the real tokens are made of.
 */
const VALUE_BANNED_RE = /[;{}<>@\\]|url\s*\(|image-set\s*\(|expression\s*\(|\/\*/i;

export function isValidOverrideName(name: string): boolean {
  return NAME_RE.test(name);
}

export function isValidOverrideValue(value: string): boolean {
  if (typeof value !== 'string') return false;
  const trimmed = value.trim();
  if (trimmed === '') return false;
  if (trimmed.length > MAX_STYLE_VALUE_LENGTH) return false;
  return !VALUE_BANNED_RE.test(trimmed);
}

/**
 * Parse the stored preference into a map safe to hand to `setProperty`.
 *
 * Invalid entries are DROPPED rather than failing the whole map: one bad value
 * written by an app must not cost the user every value they tuned by hand. A
 * corrupt or non-object payload yields an empty map for the same reason, which
 * is also what keeps it from ever breaking first paint.
 */
export function parseStyleOverrides(raw: string | null | undefined): Record<string, string> {
  if (!raw) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return {};
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {};

  const out: Record<string, string> = {};
  let n = 0;
  for (const [name, value] of Object.entries(parsed as Record<string, unknown>)) {
    if (n >= MAX_STYLE_OVERRIDES) break;
    if (!isValidOverrideName(name)) continue;
    if (typeof value !== 'string' || !isValidOverrideValue(value)) continue;
    out[name] = value.trim();
    n++;
  }
  return out;
}

/** Whether the current URL asks for the overrides to be dropped. */
export function styleResetRequested(search: string): boolean {
  return new RegExp(`[?&]${STYLE_RESET_PARAM}(?:[=&]|$)`).test(search);
}
