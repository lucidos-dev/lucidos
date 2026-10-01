/**
 * The appearance boot contract: the single source for what a device's theme, UI
 * font, ligature settings, UI scale and motion resolve to.
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

import {
  FALLBACK_FONT,
  FOLLOW_THEME,
  FONT_CATALOG,
  WORKSPACE_FONT_FALLBACKS,
  WORKSPACE_FONT_ID_PREFIX,
  WORKSPACE_FONT_LIMITS,
  type FontEntry,
  type FontGroup,
  type FontId,
} from './generated/font-catalog';
import { PART_TOKEN_PREFIX, checkPartToken } from './themeParts';

export { FALLBACK_FONT, FOLLOW_THEME, FONT_CATALOG, WORKSPACE_FONT_ID_PREFIX, WORKSPACE_FONT_LIMITS };
export type { FontEntry, FontGroup, FontId };

export type ThemeMode = 'light' | 'dark' | 'system';
/** A theme mode with `system` already resolved against the OS. */
export type ResolvedThemeMode = 'light' | 'dark';

export const THEME_MODES: readonly ThemeMode[] = ['light', 'dark', 'system'];

/** The device-scoped preference holding the theme mode. */
export const THEME_MODE_KEY = 'theme-mode';

/** Workspace-scoped mirror of `theme-mode`, read by the boot script. */
export const THEME_MODE_STORAGE_KEY = 'lucidos-theme-mode';

/** The attribute every surface paints the resolved mode on. */
export const THEME_MODE_ATTRIBUTE = 'data-theme-mode';

/** What an unset `theme-mode` preference means: follow the OS light/dark
 *  setting. A device that explicitly picked light or dark keeps its pick. */
export const DEFAULT_THEME_MODE: ThemeMode = 'system';

/** The document background per resolved mode. Painted inline on `<html>` by
 *  every surface, so it is legible before any stylesheet has been parsed. */
export const THEME_MODE_BG: Record<ResolvedThemeMode, string> = {
  light: '#ffffff',
  dark: '#07172e',
};

/** A workspace font's id (ADR 0308): `ws-` and the directory it lives in
 *  under `data/fonts/`. */
export type WorkspaceFontId = `ws-${string}`;

/** A font a surface can paint: a catalog font or a workspace font. */
export type FontKey = FontId | WorkspaceFontId;

/** A `font-family` preference value: a font, or follow the theme. */
export type FontPreference = FontKey | typeof FOLLOW_THEME;

/** What an unset `font-family` preference means: follow the active theme, which
 *  falls back to {@link FALLBACK_FONT} when the theme names no font. */
export const DEFAULT_FONT_PREFERENCE: FontPreference = FOLLOW_THEME;

/** Every `font-family` value, in the order Settings lists them. */
export const FONT_PREFERENCES: readonly FontPreference[] = [
  FOLLOW_THEME,
  ...FONT_CATALOG.map(font => font.id),
];

const FONTS_BY_ID = {} as Record<FontId, FontEntry>;
for (const font of FONT_CATALOG) FONTS_BY_ID[font.id] = font;

/** The CSS value each font resolves to. The fallback's stack is the full
 *  system-mono chain (ADR 0077). It paints before the web font decodes, and on
 *  any device where it never loads. */
export const FONT_STACKS = {} as Record<FontId, string>;
for (const font of FONT_CATALOG) FONT_STACKS[font.id] = font.stack;

export function isFontId(value: string | null | undefined): value is FontId {
  return !!value && hasOwn(FONTS_BY_ID, value);
}

export function fontEntry(id: FontId): FontEntry {
  return FONTS_BY_ID[id];
}

/** Says on `<html>` whether the UI font has a bold face: `face` or `none`.
 *  With `none`, bold text keeps the regular outlines, so the stylesheets show
 *  it as `--text-strong` instead. */
export const FONT_BOLD_ATTRIBUTE = 'data-font-bold';

/** The lightest weight that counts as bold. The engine's font catalog uses the
 *  same line (`core/fonts.rs`). */
const BOLD_WEIGHT = 600;

/** The heavy end of a face weight, one value or a range. */
function heaviestWeight(weight: string): number {
  return Number(weight.trim().split(/\s+/).pop());
}

/** A face weight reaches bold at its heaviest end. */
export function weightReachesBold(weight: string): boolean {
  return heaviestWeight(weight) >= BOLD_WEIGHT;
}

/** Whether a workspace font has a face of `style` that reaches bold. A bold
 *  italic alone leaves upright bold text to be faked. */
export function workspaceFontHasBold(
  font: WorkspaceFont,
  style: WorkspaceFontFace['style'] = 'normal',
): boolean {
  return font.faces.some(face => face.style === style && weightReachesBold(face.weight));
}

/** The weight a workspace face registers under. In a style with no bold face,
 *  the heaviest face stretches up to 900. Bold text then paints its real
 *  outlines instead of a smeared fake. Every other face keeps its own range,
 *  so two light faces never compete for the same weights. */
export function registeredFaceWeight(font: WorkspaceFont, face: WorkspaceFontFace): string {
  if (workspaceFontHasBold(font, face.style)) return face.weight;
  const top = font.faces
    .filter(other => other.style === face.style)
    .reduce((a, b) => (heaviestWeight(b.weight) > heaviestWeight(a.weight) ? b : a));
  if (top !== face) return face.weight;
  return `${face.weight.trim().split(/\s+/)[0]} 900`;
}

/** The `data-font-bold` value for the UI font a surface resolved. */
export function fontBoldMark(font: ResolvedFont): 'face' | 'none' {
  if (font.workspaceFont) return workspaceFontHasBold(font.workspaceFont) ? 'face' : 'none';
  return isFontId(font.key) && !FONTS_BY_ID[font.key].bold ? 'none' : 'face';
}

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
 * Every font without programming ligatures resolves BOTH to `normal`, which
 * leaves its rendering untouched. An unconditional `"liga" 0` would also kill
 * the `fi` and `fl` ligatures a proportional face like Inter wants.
 */
export const FONT_FEATURES_DEFAULT: FontFeaturePair = { text: 'normal', code: 'normal' };
const FONT_FEATURES_LIGATURES: FontFeaturePair = {
  text: '"liga" 0, "calt" 0',
  code: '"liga" 1, "calt" 1',
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
 * Which theme mode applies, given what each source knows. Precedence:
 *
 *   1. the server-provided value, when present and valid;
 *   2. else the `lucidos-theme-mode` localStorage value the FOUC script read;
 *   3. else the `data-theme-mode` attribute the FOUC script already applied;
 *   4. else {@link DEFAULT_THEME_MODE}, follow the OS.
 *
 * Load-bearing invariant: a MISSING server mode must NEVER clobber the value
 * the synchronous client resolver already settled on. A `prefs['theme-mode'] ||
 * 'dark'` breaks it, flipping every app iframe to dark on a device that stored
 * only `ui-scale` while localStorage said light.
 *
 * `getAttr` is a thunk so the DOM is read only as a last resort, which keeps
 * the common path side-effect-free. Returns a raw preference, and the caller
 * resolves `system` via matchMedia.
 */
export function resolveThemeModePreference(
  server: string | undefined,
  local: string | null,
  getAttr: () => string | null,
): ThemeMode {
  const valid = THEME_MODES as readonly string[];
  if (server && valid.includes(server)) return server as ThemeMode;
  if (local && valid.includes(local)) return local as ThemeMode;
  const attr = getAttr();
  return attr === 'light' || attr === 'dark' ? attr : DEFAULT_THEME_MODE;
}

/** Collapse a preference to what actually paints. `system` asks the OS. */
export function resolveThemeMode(mode: ThemeMode, prefersLight: boolean): ResolvedThemeMode {
  if (mode === 'system') return prefersLight ? 'light' : 'dark';
  return mode;
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
export const SYSTEM_THEME_MODE_SETTLE_MS = 300;

// --- Motion ---

/** The device-scoped `motion` preference. `system` follows the OS switch,
 *  `reduce` and `full` override it either way. */
export type MotionPref = 'system' | 'reduce' | 'full';

export const MOTION_PREFS: readonly MotionPref[] = ['system', 'reduce', 'full'];

export const DEFAULT_MOTION: MotionPref = 'system';

/** Workspace-scoped mirror of `motion`, read by the boot script before any
 *  module loads. */
export const MOTION_STORAGE_KEY = 'lucidos-motion';

/** The media query the resolver reads. Nothing else may read it: the resolved
 *  answer lives on `<html>` as `data-motion`, and every surface keys on that. */
export const REDUCED_MOTION_QUERY = '(prefers-reduced-motion: reduce)';

export function parseMotion(raw: string | null | undefined): MotionPref {
  return raw && (MOTION_PREFS as readonly string[]).includes(raw)
    ? raw as MotionPref
    : DEFAULT_MOTION;
}

/** Whether motion is reduced. The single definition every surface uses. */
export function resolveReducedMotion(pref: MotionPref, osReduces: boolean): boolean {
  if (pref === 'system') return osReduces;
  return pref === 'reduce';
}

/** The `data-motion` value for a resolved answer. */
export function motionAttribute(reduced: boolean): 'reduce' | 'full' {
  return reduced ? 'reduce' : 'full';
}

// --- Theme effects ---

/** The device-scoped `theme-effects` preference (ADR 0307). `reduce` drops the
 *  shadows and filters a theme puts on its parts, and keeps part colours and
 *  letter-spacing. `system` follows two OS signals, `full` ignores them. */
export type ThemeEffectsPref = 'system' | 'reduce' | 'full';

export const THEME_EFFECTS_PREFS: readonly ThemeEffectsPref[] = ['system', 'reduce', 'full'];

export const DEFAULT_THEME_EFFECTS: ThemeEffectsPref = 'system';

/** Workspace-scoped mirror of `theme-effects`, read by the boot script. */
export const THEME_EFFECTS_STORAGE_KEY = 'lucidos-theme-effects';

/** The OS signals `system` follows. A glow softens glyph edges, which is what
 *  a user asking for more contrast or less transparency is avoiding. A browser
 *  that does not know a query reports no match, so that half reads as false. */
export const REDUCED_TRANSPARENCY_QUERY = '(prefers-reduced-transparency: reduce)';
export const MORE_CONTRAST_QUERY = '(prefers-contrast: more)';

export function parseThemeEffects(raw: string | null | undefined): ThemeEffectsPref {
  return raw && (THEME_EFFECTS_PREFS as readonly string[]).includes(raw)
    ? raw as ThemeEffectsPref
    : DEFAULT_THEME_EFFECTS;
}

/** Whether theme effects are reduced. The single definition every surface uses. */
export function resolveReducedThemeEffects(
  pref: ThemeEffectsPref,
  osReducesTransparency: boolean,
  osPrefersMoreContrast: boolean,
): boolean {
  if (pref === 'system') return osReducesTransparency || osPrefersMoreContrast;
  return pref === 'reduce';
}

/** The `data-theme-effects` value for a resolved answer. */
export function themeEffectsAttribute(reduced: boolean): 'reduce' | 'full' {
  return reduced ? 'reduce' : 'full';
}

/** Device-local position of the diagnostic Animation speed slider, -10..10. */
export const ANIMATION_SPEED_STORAGE_KEY = 'lucidos-animation-speed-slider';
export const ANIMATION_SPEED_MIN = -10;
export const ANIMATION_SPEED_MAX = 10;

export function parseAnimationSpeed(raw: string | null | undefined): number {
  const n = parseInt(raw ?? '', 10);
  if (isNaN(n)) return 0;
  return Math.max(ANIMATION_SPEED_MIN, Math.min(ANIMATION_SPEED_MAX, n));
}

/** Slider position to speed multiplier, 0.1x..10x, via 10^(v/10). */
export function speedMultiplierFor(position: number): number {
  return Math.pow(10, position / 10);
}

/**
 * What reduced motion multiplies every scaled duration by.
 *
 * Above zero on purpose. A 0s transition never starts, so it never fires
 * `transitionend`, and code waiting on one would hang until its fallback.
 * Small enough that the slowest duration token ends inside one frame.
 */
export const REDUCED_MOTION_DURATION_SCALE = 0.001;

/** What every animated duration is multiplied by: the reciprocal of the speed,
 *  or the reduced-motion constant, which wins over the slider. */
export function durationScaleFor(sliderPosition: number, reduced: boolean): number {
  return reduced ? REDUCED_MOTION_DURATION_SCALE : 1 / speedMultiplierFor(sliderPosition);
}

/**
 * Which font paints the UI. The single definition every surface calls.
 *
 *   1. an explicit font id the user picked;
 *   2. else the font the active theme suggests;
 *   3. else {@link FALLBACK_FONT}.
 *
 * An unset, unknown or `theme` stored value all follow the theme. Every surface
 * resolves the KEY once and then reads both maps with it, rather than
 * defaulting each lookup separately. A key absent from the stack map would take
 * the default STACK while the feature lookup fell through to `normal`. And
 * `normal` turns Fira Code's ligatures back on for prose.
 */
export function resolveFontKey(
  stored: string | null | undefined,
  themeFonts: ThemeFonts,
  known: readonly WorkspaceFont[] = [],
): FontKey {
  const usable = (value: string | null | undefined): value is FontKey =>
    isFontId(value) || known.some(entry => entry.id === value);
  if (stored !== FOLLOW_THEME && usable(stored)) return stored;
  return usable(themeFonts.ui) ? themeFonts.ui : FALLBACK_FONT;
}

/** The ligature pair for a font. Only fonts that ship programming ligatures
 *  get anything but `normal`. */
export function fontFeaturesFor(font: FontKey, known: readonly WorkspaceFont[] = []): FontFeaturePair {
  const ligatures = isFontId(font)
    ? FONTS_BY_ID[font].ligatures
    : known.some(entry => entry.id === font && entry.ligatures);
  return ligatures ? FONT_FEATURES_LIGATURES : FONT_FEATURES_DEFAULT;
}

/** The CSS value a font resolves to. A workspace font missing from `known`
 *  takes the fallback's stack, as an unknown font always has. */
export function fontStackFor(font: FontKey, known: readonly WorkspaceFont[] = []): string {
  if (isFontId(font)) return FONT_STACKS[font];
  return known.find(entry => entry.id === font)?.stack ?? FONT_STACKS[FALLBACK_FONT];
}

/** What a surface paints for its UI font: the resolved key, the stack and the
 *  ligature pair read with it, and the workspace font to register, if any. */
export interface ResolvedFont {
  key: FontKey;
  stack: string;
  features: FontFeaturePair;
  workspaceFont: WorkspaceFont | null;
}

/** {@link resolveFontKey}, then everything read with the key it settled on.
 *  `known` is every workspace font this surface has an entry for. */
export function resolveFont(
  stored: string | null | undefined,
  themeFonts: ThemeFonts,
  known: readonly WorkspaceFont[] = [],
): ResolvedFont {
  const key = resolveFontKey(stored, themeFonts, known);
  return {
    key,
    stack: fontStackFor(key, known),
    features: fontFeaturesFor(key, known),
    workspaceFont: known.find(entry => entry.id === key) ?? null,
  };
}

/**
 * An OWN key, never an inherited one.
 *
 * Every lookup here is keyed by a value out of localStorage, and `in` (or a
 * bare index) walks the prototype chain: `'toString' in FONT_STACKS` is true,
 * so a stored `toString` would resolve to a FONT KEY, and the caller would then
 * write `Object.prototype.toString`'s source text into `--font-ui`.
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

/**
 * Names a style override may never set, whatever its value (ADR 0309).
 *
 * | Names | Why |
 * |---|---|
 * | `--protected-*` | the palette protected surfaces read; only the engine's resolved theme carries it |
 * | `--z-*` | the stacking scale, which decides what paints over an approval |
 * | the UI font tokens | the user's own font pick, which a theme may not set either |
 * | `--part-screen-background-image` | the scanlines under protected text, which the engine clamps that palette against (ADR 0313) |
 * | `--user-ui-scale` | the root size every protected `rem` rides, set by the UI scale preference |
 */
const RESERVED_OVERRIDE_NAME_RE =
  /^--(?:protected-|z-)|^--part-screen-background-image$|^--font(?:-ui|-family|-features-text|-features-code)?$|^--user-ui-scale$/;

export function isReservedOverrideName(name: string): boolean {
  return RESERVED_OVERRIDE_NAME_RE.test(name);
}

/** The theme tokens that feed a `box-shadow` or `text-shadow`. The engine's
 *  catalog lists the same set, pinned by `theme-validation-cases.json`. */
export const SHADOW_OVERRIDE_TOKENS: readonly string[] = [
  '--text-glow', '--focus-pill-glow', '--focus-ring',
  '--shadow-sm', '--shadow-md', '--shadow-lg', '--shadow-up',
];

/** How far a shadow may paint past its box, in px, and the functions it may
 *  call. The engine holds a theme to the same (`validate_shadow_reach`). */
const MAX_SHADOW_PX = 32;
const PX_PER_REM = 16;
const SHADOW_COLOUR_FUNCTIONS = new Set([
  'rgb', 'rgba', 'hsl', 'hsla', 'hwb', 'lab', 'lch', 'oklab', 'oklch', 'color', 'color-mix',
]);

/** Split at `isSep` characters outside parentheses, dropping empty pieces. */
function splitTopLevel(value: string, isSep: (c: string) => boolean): string[] {
  const parts: string[] = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < value.length; i++) {
    const c = value[i];
    if (c === '(') depth++;
    else if (c === ')') depth--;
    else if (depth === 0 && isSep(c)) {
      parts.push(value.slice(start, i));
      start = i + 1;
    }
  }
  parts.push(value.slice(start));
  return parts.map(p => p.trim()).filter(p => p !== '');
}

/** A word that is exactly one call, `name(args)` with balanced parentheses. */
function wholeCallName(word: string): string | null {
  const m = /^([a-z-]+)\(([\s\S]*)\)$/.exec(word);
  if (!m) return null;
  let depth = 0;
  for (const c of m[2]) {
    if (c === '(') depth++;
    else if (c === ')' && --depth < 0) return null;
  }
  return depth === 0 ? m[1] : null;
}

/** One word of a shadow in px: `null` for a keyword or colour, `undefined`
 *  when its size cannot be checked. CSS ends a word's first token at a `+`,
 *  and at a sign after a number, so `0+999px` is two lengths. */
function shadowLengthPx(word: string): number | null | undefined {
  if (!/^[0-9.+-]/.test(word)) return word.includes('+') ? undefined : null;
  const m = /^([+-]?(?:\d+\.?\d*|\.\d+))(.*)$/.exec(word);
  if (!m) return undefined;
  const length = Number(m[1]);
  if (!Number.isFinite(length)) return undefined;
  const unit = m[2];
  if (unit === '' && length === 0) return 0;
  if (unit === 'px') return length;
  if (unit === 'rem' || unit === 'em') return length * PX_PER_REM;
  return undefined;
}

/** Whether a shadow stays within reach of its box: each layer's larger
 *  offset, plus its spread, plus half its blur. */
export function shadowWithinReach(value: string): boolean {
  for (const layer of splitTopLevel(value, c => c === ',')) {
    const lengths: number[] = [];
    for (const raw of splitTopLevel(layer, c => /\s/.test(c))) {
      const word = raw.toLowerCase();
      if (word.includes('(')) {
        const name = wholeCallName(word);
        if (name === null || !SHADOW_COLOUR_FUNCTIONS.has(name)) return false;
        continue;
      }
      const px = shadowLengthPx(word);
      if (px === undefined) return false;
      if (px !== null) lengths.push(px);
    }
    if (lengths.length === 0) continue;
    if (lengths.length === 1 || lengths.length > 4) return false;
    const [x, y, blur = 0, spread = 0] = lengths;
    const reach = Math.max(Math.abs(x), Math.abs(y)) + Math.max(spread, 0) + Math.max(blur, 0) / 2;
    if (reach > MAX_SHADOW_PX) return false;
  }
  return true;
}

/** Whether a style override may set `name` to `value`. The one test both
 *  realms and the boot script apply. */
export function isAllowedOverride(name: string, value: string): boolean {
  if (!isValidOverrideName(name) || isReservedOverrideName(name)) return false;
  if (!isValidOverrideValue(value)) return false;
  return !SHADOW_OVERRIDE_TOKENS.includes(name) || shadowWithinReach(value.trim());
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
  const map = sanitizeTokenMap(parseJson(raw));
  for (const [name, value] of Object.entries(map)) {
    if (!isAllowedOverride(name, value)) delete map[name];
  }
  return map;
}

function parseJson(raw: string | null | undefined): unknown {
  if (!raw) return null;
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

/** Keep only the entries safe to hand to `setProperty`, up to the cap. The one
 *  gate every token map passes: style overrides and themes alike. A part token
 *  must also pass the part grammar, and lands in its canonical form. */
function sanitizeTokenMap(value: unknown): Record<string, string> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const out: Record<string, string> = {};
  let n = 0;
  for (const [name, token] of Object.entries(value as Record<string, unknown>)) {
    if (n >= MAX_STYLE_OVERRIDES) break;
    if (!isValidOverrideName(name)) continue;
    if (typeof token !== 'string' || !isValidOverrideValue(token)) continue;
    if (name.startsWith(PART_TOKEN_PREFIX)) {
      const part = checkPartToken(name, token);
      if (!('ok' in part)) continue;
      out[name] = part.ok;
    } else {
      out[name] = token.trim();
    }
    n++;
  }
  return out;
}

/** Whether the current URL asks for the overrides to be dropped. */
export function styleResetRequested(search: string): boolean {
  return new RegExp(`[?&]${STYLE_RESET_PARAM}(?:[=&]|$)`).test(search);
}

// --- Themes ---
//
// A theme is a named set of token values (`docs/plans/2026-09-26-looks.md`). The
// engine resolves it, derivation included, and serves one map per theme mode.
// Every surface here only picks the map for the mode it painted and applies it,
// between the stylesheet and the style overrides. The maps pass the same gate
// as the overrides, because a workspace theme is a file any app can write.

/** The device-scoped preference naming the active theme. */
export const THEME_KEY = 'theme';

/** What an unset `theme` means: the stylesheet as shipped, no inline tokens. */
export const DEFAULT_THEME_ID = 'lucidos';

/** Workspace-scoped localStorage key: the active theme, resolved, as JSON, for
 *  the boot script. The engine seeds an app frame the same shape. */
export const THEME_STORAGE_KEY = 'lucidos-theme-resolved';

/** The seed key the engine prepends for an app frame (`api/sdk_prefs.rs`). */
export const THEME_SEED_KEY = 'theme_resolved';

/** The fonts a theme suggests: catalog ids and well-formed workspace font ids
 *  survive parsing. Neither kind makes a third-party request (ADR 0303, ADR
 *  0308). A workspace font paints only where a surface holds its entry. */
export interface ThemeFonts {
  ui?: FontKey;
  mono?: FontKey;
}

/** One token map per theme mode. */
export type ThemeModeMaps = Record<ResolvedThemeMode, Record<string, string>>;

/** A theme as every surface paints it: one token map per theme mode, the fonts
 *  it suggests, and the entry of each workspace font among them. The engine's
 *  `ResolvedTheme`, same shape. */
export interface ResolvedTheme extends ThemeModeMaps {
  fonts: ThemeFonts;
  workspace_fonts: WorkspaceFont[];
}

export const EMPTY_THEME: ResolvedTheme = { dark: {}, light: {}, fonts: {}, workspace_fonts: [] };

/** Parse a stored or served resolved theme. Anything malformed parses to the
 *  empty theme, so a corrupt cache can never break first paint. */
export function parseResolvedTheme(raw: string | null | undefined): ResolvedTheme {
  return sanitizeResolvedTheme(parseJson(raw));
}

/** The same gate for a theme already parsed, such as one the API served. */
export function sanitizeResolvedTheme(value: unknown): ResolvedTheme {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return EMPTY_THEME;
  const theme = value as Record<string, unknown>;
  return {
    dark: sanitizeTokenMap(theme.dark),
    light: sanitizeTokenMap(theme.light),
    fonts: sanitizeThemeFonts(theme.fonts),
    workspace_fonts: sanitizeWorkspaceFonts(theme.workspace_fonts),
  };
}

function sanitizeThemeFonts(value: unknown): ThemeFonts {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const fonts = value as Record<string, unknown>;
  const out: ThemeFonts = {};
  for (const slot of ['ui', 'mono'] as const) {
    const id = fonts[slot];
    if (typeof id === 'string' && (isFontId(id) || isWorkspaceFontId(id))) out[slot] = id;
  }
  return out;
}

/** Every name either mode sets: what a realm may have laid inline from a
 *  cache, and so must be able to remove. */
export function themeTokenNames(theme: ThemeModeMaps): string[] {
  return [...new Set([...Object.keys(theme.dark), ...Object.keys(theme.light)])];
}

/** Where a realm writes its inline tokens: `<html>`'s style declaration. */
export interface InlineStyle {
  setProperty(name: string, value: string): void;
  removeProperty(name: string): void;
}

/**
 * Lay `next` inline and clear each name in `previous` that `next` drops.
 *
 * A dropped name falls back to `beneath`, the layer under this one. So
 * clearing a style override uncovers the theme's value rather than deleting it.
 * Returns the names now set, for the next call's `previous`.
 */
export function replaceInlineTokens(
  style: InlineStyle,
  previous: readonly string[],
  next: Record<string, string>,
  beneath: Record<string, string> = {},
): string[] {
  for (const name of previous) {
    if (name in next) continue;
    if (hasOwn(beneath, name)) style.setProperty(name, beneath[name]);
    else style.removeProperty(name);
  }
  for (const name of Object.keys(next)) style.setProperty(name, next[name]);
  return Object.keys(next);
}

/** The literal document background a theme paints in a mode, or null to keep
 *  {@link THEME_MODE_BG}. Only a hex literal qualifies: this value is painted before
 *  any stylesheet resolves a `var()`. */
export function themeBackground(tokens: Record<string, string>): string | null {
  const bg = tokens['--bg-primary'];
  return bg && /^#(?:[0-9a-f]{3}|[0-9a-f]{6})$/i.test(bg) ? bg : null;
}

// --- Workspace fonts ---
//
// A workspace font is one the user, the agent or a plugin installed under
// `data/fonts/<slug>/` (ADR 0308). The engine checks it and serves its entry.
// A client still rebuilds the entry rather than trust it: an entry reaches a
// surface from local storage too, and storage can be forged. The stack is
// rebuilt from the id and the group, and every face path must name a font file
// in the font's own directory. So no string from the wire reaches CSS, and no
// face can point anywhere but the local engine's `/data` mount.

/** One face of a workspace font. `path` is relative to the workspace `data/`. */
export interface WorkspaceFontFace {
  path: string;
  weight: string;
  style: 'normal' | 'italic';
}

/** A workspace font as a surface registers and paints it. The engine's
 *  `WorkspaceFont`, less what no surface reads. */
export interface WorkspaceFont {
  id: WorkspaceFontId;
  label: string;
  /** The family its faces register under. It is the id. */
  family: WorkspaceFontId;
  stack: string;
  group: FontGroup;
  ligatures: boolean;
  faces: WorkspaceFontFace[];
}

/** Workspace-scoped localStorage key: the entry of the workspace font the
 *  device picked, as JSON, for the boot script. */
export const WORKSPACE_FONT_STORAGE_KEY = 'lucidos-workspace-font';

/** The seed key the engine prepends for an app frame (`api/sdk_prefs.rs`). */
export const WORKSPACE_FONT_SEED_KEY = 'workspace_font';

const WORKSPACE_FONT_SLUG = '[a-z0-9]+(?:-[a-z0-9]+)*';
const WORKSPACE_FONT_ID = new RegExp(`^${WORKSPACE_FONT_ID_PREFIX}${WORKSPACE_FONT_SLUG}$`);
const WORKSPACE_FONT_ID_MAX = WORKSPACE_FONT_ID_PREFIX.length + WORKSPACE_FONT_LIMITS.slug;
const FACE_FILE = /^[A-Za-z0-9_-][A-Za-z0-9._-]{0,99}$/;
const FACE_EXTENSION = /\.(?:woff2|woff|ttf|otf)$/i;
const FACE_WEIGHT = /^(\d{1,4})(?: (\d{1,4}))?$/;
const FONT_GROUPS: readonly FontGroup[] = ['sans', 'serif', 'mono'];

export function isWorkspaceFontId(value: string | null | undefined): value is WorkspaceFontId {
  return !!value && value.length <= WORKSPACE_FONT_ID_MAX && WORKSPACE_FONT_ID.test(value);
}

/** The stack a workspace font paints with: its own family, then the fixed
 *  chain for its group. */
export function workspaceFontStack(id: WorkspaceFontId, group: FontGroup): string {
  return `'${id}', ${WORKSPACE_FONT_FALLBACKS[group]}`;
}

function isFaceWeight(weight: string): boolean {
  const match = FACE_WEIGHT.exec(weight);
  if (!match) return false;
  const min = Number(match[1]);
  const max = match[2] === undefined ? min : Number(match[2]);
  return min >= 1 && max <= 1000 && min <= max;
}

function sanitizeFace(value: unknown, slug: string): WorkspaceFontFace | null {
  if (!value || typeof value !== 'object') return null;
  const face = value as Record<string, unknown>;
  const { path, weight, style } = face;
  if (typeof path !== 'string' || typeof weight !== 'string') return null;
  const prefix = `fonts/${slug}/`;
  const file = path.startsWith(prefix) ? path.slice(prefix.length) : '';
  if (!FACE_FILE.test(file) || !FACE_EXTENSION.test(file)) return null;
  if (!isFaceWeight(weight)) return null;
  if (style !== 'normal' && style !== 'italic') return null;
  return { path, weight, style };
}

/** A workspace font entry rebuilt from what the engine served, or null when
 *  any part of it is malformed. */
export function sanitizeWorkspaceFont(value: unknown): WorkspaceFont | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const font = value as Record<string, unknown>;
  const { id, group, faces } = font;
  if (typeof id !== 'string' || !isWorkspaceFontId(id)) return null;
  if (typeof group !== 'string' || !(FONT_GROUPS as readonly string[]).includes(group)) return null;
  if (!Array.isArray(faces) || faces.length === 0 || faces.length > WORKSPACE_FONT_LIMITS.faces) {
    return null;
  }
  const slug = id.slice(WORKSPACE_FONT_ID_PREFIX.length);
  const clean: WorkspaceFontFace[] = [];
  for (const face of faces) {
    const checked = sanitizeFace(face, slug);
    if (!checked) return null;
    clean.push(checked);
  }
  const label = typeof font.label === 'string' && font.label.trim()
    ? font.label.slice(0, WORKSPACE_FONT_LIMITS.label)
    : id;
  return {
    id,
    label,
    family: id,
    stack: workspaceFontStack(id, group as FontGroup),
    group: group as FontGroup,
    ligatures: font.ligatures === true,
    faces: clean,
  };
}

/** Every well-formed entry in a served or stored list, up to the cap. */
export function sanitizeWorkspaceFonts(value: unknown): WorkspaceFont[] {
  if (!Array.isArray(value)) return [];
  const out: WorkspaceFont[] = [];
  for (const entry of value) {
    if (out.length >= WORKSPACE_FONT_LIMITS.fonts) break;
    const font = sanitizeWorkspaceFont(entry);
    if (font && !out.some(known => known.id === font.id)) out.push(font);
  }
  return out;
}

/** Parse a stored or seeded workspace font entry. */
export function parseWorkspaceFont(raw: string | null | undefined): WorkspaceFont | null {
  return sanitizeWorkspaceFont(parseJson(raw));
}
