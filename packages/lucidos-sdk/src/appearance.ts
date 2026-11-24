
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
import {
  PREF_FONT_FAMILY, PREF_MOTION, PREF_THEME, PREF_THEME_EFFECTS, PREF_THEME_MODE, PREF_UI_SCALE,
  type PreferenceValues,
} from './generated/preference-catalog';
import {
  MAX_RESOLVED_TOKENS, MAX_SHADOW_PX, MAX_VALUE_LENGTH, PX_PER_REM, SHADOW_COLOUR_FUNCTIONS,
} from './generated/theme-parts';
import { PART_TOKEN_PREFIX, checkPartToken } from './themeParts';

export { FALLBACK_FONT, FOLLOW_THEME, FONT_CATALOG, WORKSPACE_FONT_ID_PREFIX, WORKSPACE_FONT_LIMITS };
export type { FontEntry, FontGroup, FontId };

export type ThemeMode = PreferenceValues<'theme-mode'>;
export type ResolvedThemeMode = Exclude<ThemeMode, 'system'>;

export const THEME_MODES: readonly ThemeMode[] = PREF_THEME_MODE.values;

export const THEME_MODE_KEY = PREF_THEME_MODE.key;

export const THEME_MODE_STORAGE_KEY = 'lucidos-theme-mode';

export const THEME_MODE_ATTRIBUTE = 'data-theme-mode';

export const DEFAULT_THEME_MODE: ThemeMode = PREF_THEME_MODE.fallback;

export const THEME_MODE_BG: Record<ResolvedThemeMode, string> = {
  light: '#ffffff',
  dark: '#07172e',
};

export const THEME_MODE_HEADER_TOP: Record<ResolvedThemeMode, string> = {
  light: '#1a6fd0',
  dark: '#15549e',
};

export function statusBarColor(inlineHeaderTop: string, resolved: ResolvedThemeMode): string {
  return hexLiteral(inlineHeaderTop.trim()) ?? THEME_MODE_HEADER_TOP[resolved];
}

export type WorkspaceFontId = `ws-${string}`;

export type FontKey = FontId | WorkspaceFontId;

export type FontPreference = FontKey | typeof FOLLOW_THEME;

export const FONT_PREFERENCES: readonly FontPreference[] = [
  FOLLOW_THEME,
  ...FONT_CATALOG.map(font => font.id),
];

const FONTS_BY_ID = {} as Record<FontId, FontEntry>;
for (const font of FONT_CATALOG) FONTS_BY_ID[font.id] = font;

export const FONT_STACKS = {} as Record<FontId, string>;
for (const font of FONT_CATALOG) FONT_STACKS[font.id] = font.stack;

export function isFontId(value: string | null | undefined): value is FontId {
  return !!value && hasOwn(FONTS_BY_ID, value);
}

export function fontEntry(id: FontId): FontEntry {
  return FONTS_BY_ID[id];
}

export const FONT_BOLD_ATTRIBUTE = 'data-font-bold';

const BOLD_WEIGHT = 600;

function heaviestWeight(weight: string): number {
  return Number(weight.trim().split(/\s+/).pop());
}

export function weightReachesBold(weight: string): boolean {
  return heaviestWeight(weight) >= BOLD_WEIGHT;
}

export function workspaceFontHasBold(
  font: WorkspaceFont,
  style: WorkspaceFontFace['style'] = 'normal',
): boolean {
  return font.faces.some(face => face.style === style && weightReachesBold(face.weight));
}

export function registeredFaceWeight(font: WorkspaceFont, face: WorkspaceFontFace): string {
  if (workspaceFontHasBold(font, face.style)) return face.weight;
  const top = font.faces
    .filter(other => other.style === face.style)
    .reduce((a, b) => (heaviestWeight(b.weight) > heaviestWeight(a.weight) ? b : a));
  if (top !== face) return face.weight;
  return `${face.weight.trim().split(/\s+/)[0]} 900`;
}

export function fontBoldMark(font: ResolvedFont): 'face' | 'none' {
  if (font.workspaceFont) return workspaceFontHasBold(font.workspaceFont) ? 'face' : 'none';
  return isFontId(font.key) && !FONTS_BY_ID[font.key].bold ? 'none' : 'face';
}

export interface FontFeaturePair {
  text: string;
  code: string;
}

export const FONT_FEATURES_DEFAULT: FontFeaturePair = { text: 'normal', code: 'normal' };
const FONT_FEATURES_LIGATURES: FontFeaturePair = {
  text: '"liga" 0, "calt" 0',
  code: '"liga" 1, "calt" 1',
};

export const UI_SCALE_MIN = PREF_UI_SCALE.min;
export const UI_SCALE_MAX = PREF_UI_SCALE.max;
export const UI_SCALE_STEP = 12.5;

export const LEGACY_UI_SCALES: Record<string, number> = {
  small: 100,
  medium: 112.5,
  large: 125,
};

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

export function resolveThemeMode(mode: ThemeMode, prefersLight: boolean): ResolvedThemeMode {
  if (mode === 'system') return prefersLight ? 'light' : 'dark';
  return mode;
}

export function repaintThemeMode(
  next: ThemeMode,
  previous: ThemeMode,
  painted: string | null,
  osPrefersLight: () => boolean,
): ResolvedThemeMode {
  if (next !== 'system') return next;
  if (previous === 'system' && (painted === 'light' || painted === 'dark')) return painted;
  return resolveThemeMode(next, osPrefersLight());
}

export const SYSTEM_THEME_MODE_SETTLE_MS = 300;

export const SYSTEM_THEME_MODE_CONFIRM_MS = 1000;

export interface SystemThemeModePage {
  visible(): boolean;
  moved(): boolean;
  apply(): void;
}

export function followSystemThemeMode(page: SystemThemeModePage): { schedule(): void; cancel(): void } {
  let timer: ReturnType<typeof setTimeout> | null = null;
  const cancel = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
  };
  const read = (confirming: boolean) => {
    timer = null;
    if (!page.visible() || !page.moved()) return;
    if (confirming) page.apply();
    else timer = setTimeout(() => read(true), SYSTEM_THEME_MODE_CONFIRM_MS);
  };
  const schedule = () => {
    if (!page.visible()) cancel();
    else if (timer === null) timer = setTimeout(() => read(false), SYSTEM_THEME_MODE_SETTLE_MS);
  };
  return { schedule, cancel };
}


export type MotionPref = PreferenceValues<'motion'>;

export const MOTION_PREFS: readonly MotionPref[] = PREF_MOTION.values;

export const DEFAULT_MOTION: MotionPref = PREF_MOTION.fallback;

export const MOTION_STORAGE_KEY = 'lucidos-motion';

export const REDUCED_MOTION_QUERY = '(prefers-reduced-motion: reduce)';

export function parseMotion(raw: string | null | undefined): MotionPref {
  return raw && (MOTION_PREFS as readonly string[]).includes(raw)
    ? raw as MotionPref
    : DEFAULT_MOTION;
}

export function resolveReducedMotion(pref: MotionPref, osReduces: boolean): boolean {
  if (pref === 'system') return osReduces;
  return pref === 'reduce';
}

export function motionAttribute(reduced: boolean): 'reduce' | 'full' {
  return reduced ? 'reduce' : 'full';
}


export type ThemeEffectsPref = PreferenceValues<'theme-effects'>;

export const THEME_EFFECTS_PREFS: readonly ThemeEffectsPref[] = PREF_THEME_EFFECTS.values;

export const DEFAULT_THEME_EFFECTS: ThemeEffectsPref = PREF_THEME_EFFECTS.fallback;

export const THEME_EFFECTS_STORAGE_KEY = 'lucidos-theme-effects';

export const REDUCED_TRANSPARENCY_QUERY = '(prefers-reduced-transparency: reduce)';
export const MORE_CONTRAST_QUERY = '(prefers-contrast: more)';

export function parseThemeEffects(raw: string | null | undefined): ThemeEffectsPref {
  return raw && (THEME_EFFECTS_PREFS as readonly string[]).includes(raw)
    ? raw as ThemeEffectsPref
    : DEFAULT_THEME_EFFECTS;
}

export function resolveReducedThemeEffects(
  pref: ThemeEffectsPref,
  osReducesTransparency: boolean,
  osPrefersMoreContrast: boolean,
): boolean {
  if (pref === 'system') return osReducesTransparency || osPrefersMoreContrast;
  return pref === 'reduce';
}

export function themeEffectsAttribute(reduced: boolean): 'reduce' | 'full' {
  return reduced ? 'reduce' : 'full';
}

export const ANIMATION_SPEED_STORAGE_KEY = 'lucidos-animation-speed-slider';
export const ANIMATION_SPEED_MIN = -10;
export const ANIMATION_SPEED_MAX = 10;

export function parseAnimationSpeed(raw: string | null | undefined): number {
  const n = parseInt(raw ?? '', 10);
  if (isNaN(n)) return 0;
  return Math.max(ANIMATION_SPEED_MIN, Math.min(ANIMATION_SPEED_MAX, n));
}

export function speedMultiplierFor(position: number): number {
  return Math.pow(10, position / 10);
}

export const REDUCED_MOTION_DURATION_SCALE = 0.001;

export function durationScaleFor(sliderPosition: number, reduced: boolean): number {
  return reduced ? REDUCED_MOTION_DURATION_SCALE : 1 / speedMultiplierFor(sliderPosition);
}

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

export function fontFeaturesFor(font: FontKey, known: readonly WorkspaceFont[] = []): FontFeaturePair {
  const ligatures = isFontId(font)
    ? FONTS_BY_ID[font].ligatures
    : known.some(entry => entry.id === font && entry.ligatures);
  return ligatures ? FONT_FEATURES_LIGATURES : FONT_FEATURES_DEFAULT;
}

export function fontStackFor(font: FontKey, known: readonly WorkspaceFont[] = []): string {
  if (isFontId(font)) return FONT_STACKS[font];
  return known.find(entry => entry.id === font)?.stack ?? FONT_STACKS[FALLBACK_FONT];
}

export interface ResolvedFont {
  key: FontKey;
  stack: string;
  features: FontFeaturePair;
  workspaceFont: WorkspaceFont | null;
}

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

function hasOwn(obj: object, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(obj, key);
}

export function clampUiScale(scale: number): number {
  const snapped = Math.round(scale / UI_SCALE_STEP) * UI_SCALE_STEP;
  return Math.max(UI_SCALE_MIN, Math.min(UI_SCALE_MAX, snapped));
}

export function parseUiScale(raw: string | null | undefined): number | null {
  if (!raw) return null;
  const n = hasOwn(LEGACY_UI_SCALES, raw) ? LEGACY_UI_SCALES[raw] : parseFloat(raw);
  if (isNaN(n)) return null;
  return clampUiScale(n);
}


export const STYLE_OVERRIDES_STORAGE_KEY = 'lucidos-style-overrides';

export const STYLE_RESET_PARAM = 'style-reset';

export const MAX_STYLE_OVERRIDES = MAX_RESOLVED_TOKENS;

export const MAX_STYLE_VALUE_LENGTH = MAX_VALUE_LENGTH;

const NAME_RE = /^--[a-z][a-z0-9-]*$/;

const VALUE_BANNED_RE = /[;{}<>@\\]|url\s*\(|image-set\s*\(|expression\s*\(|\/\*/i;

export function isValidOverrideName(name: string): boolean {
  return NAME_RE.test(name);
}

const RESERVED_OVERRIDE_NAME_RE =
  /^--(?:protected-|z-)|^--part-screen-background-image$|^--font(?:-ui|-family|-features-text|-features-code)?$|^--user-ui-scale$/;

export function isReservedOverrideName(name: string): boolean {
  return RESERVED_OVERRIDE_NAME_RE.test(name);
}

export const SHADOW_OVERRIDE_TOKENS: readonly string[] = [
  '--text-glow', '--focus-pill-glow', '--focus-ring',
  '--shadow-sm', '--shadow-md', '--shadow-lg', '--shadow-up',
];

const SHADOW_COLOUR_FUNCTION_SET = new Set(SHADOW_COLOUR_FUNCTIONS);

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

export function shadowWithinReach(value: string): boolean {
  for (const layer of splitTopLevel(value, c => c === ',')) {
    const lengths: number[] = [];
    for (const raw of splitTopLevel(layer, c => /\s/.test(c))) {
      const word = raw.toLowerCase();
      if (word.includes('(')) {
        const name = wholeCallName(word);
        if (name === null || !SHADOW_COLOUR_FUNCTION_SET.has(name)) return false;
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

export function styleResetRequested(search: string): boolean {
  return new RegExp(`[?&]${STYLE_RESET_PARAM}(?:[=&]|$)`).test(search);
}


export const THEME_KEY = PREF_THEME.key;

export const THEME_STORAGE_KEY = 'lucidos-theme-resolved';

export const THEME_SEED_KEY = 'theme_resolved';

export interface ThemeFonts {
  ui?: FontKey;
  mono?: FontKey;
}

export type ThemeModeMaps = Record<ResolvedThemeMode, Record<string, string>>;

export interface ResolvedTheme extends ThemeModeMaps {
  fonts: ThemeFonts;
  workspace_fonts: WorkspaceFont[];
}

export const EMPTY_THEME: ResolvedTheme = { dark: {}, light: {}, fonts: {}, workspace_fonts: [] };

export function parseResolvedTheme(raw: string | null | undefined): ResolvedTheme {
  return sanitizeResolvedTheme(parseJson(raw));
}

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

export function themeTokenNames(theme: ThemeModeMaps): string[] {
  return [...new Set([...Object.keys(theme.dark), ...Object.keys(theme.light)])];
}

export interface InlineStyle {
  setProperty(name: string, value: string): void;
  removeProperty(name: string): void;
}

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

export function themeBackground(tokens: Record<string, string>): string | null {
  return hexLiteral(tokens['--bg-primary']);
}

function hexLiteral(value: string | undefined): string | null {
  return value && /^#(?:[0-9a-f]{3}|[0-9a-f]{6})$/i.test(value) ? value : null;
}


export interface WorkspaceFontFace {
  path: string;
  weight: string;
  style: 'normal' | 'italic';
}

export interface WorkspaceFont {
  id: WorkspaceFontId;
  label: string;
  family: WorkspaceFontId;
  stack: string;
  group: FontGroup;
  ligatures: boolean;
  faces: WorkspaceFontFace[];
}

export const WORKSPACE_FONT_STORAGE_KEY = 'lucidos-workspace-font';

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

export function parseWorkspaceFont(raw: string | null | undefined): WorkspaceFont | null {
  return sanitizeWorkspaceFont(parseJson(raw));
}


export const APPEARANCE_CHANNEL = 'appearance';

export const FONT_FAMILY_STORAGE_KEY = 'lucidos-font-family';
export const UI_SCALE_STORAGE_KEY = 'lucidos-ui-scale';

const APPEARANCE_PUSH_SOURCES: ReadonlyArray<readonly [string, string]> = [
  [THEME_MODE_KEY, THEME_MODE_STORAGE_KEY],
  [THEME_SEED_KEY, THEME_STORAGE_KEY],
  [PREF_FONT_FAMILY.key, FONT_FAMILY_STORAGE_KEY],
  [WORKSPACE_FONT_SEED_KEY, WORKSPACE_FONT_STORAGE_KEY],
  [PREF_UI_SCALE.key, UI_SCALE_STORAGE_KEY],
  ['style_overrides', STYLE_OVERRIDES_STORAGE_KEY],
  [PREF_MOTION.key, MOTION_STORAGE_KEY],
  [PREF_THEME_EFFECTS.key, THEME_EFFECTS_STORAGE_KEY],
];

export const PAINTED_THEME_MODE_PUSH_KEY = 'painted_theme_mode';

const APPEARANCE_PUSH_KEYS: readonly string[] = [
  ...APPEARANCE_PUSH_SOURCES.map(([key]) => key),
  PAINTED_THEME_MODE_PUSH_KEY,
];

export function readAppearancePush(
  get: (key: string) => string | null,
  paintedThemeMode: string | null,
): Record<string, string> {
  const push: Record<string, string> = {};
  for (const [key, storageKey] of APPEARANCE_PUSH_SOURCES) {
    const value = get(storageKey);
    if (value !== null) push[key] = value;
  }
  if (paintedThemeMode !== null) push[PAINTED_THEME_MODE_PUSH_KEY] = paintedThemeMode;
  return push;
}

export function sanitizeAppearancePush(value: unknown): Record<string, string> | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const raw = value as Record<string, unknown>;
  const push: Record<string, string> = {};
  for (const key of APPEARANCE_PUSH_KEYS) {
    const entry = raw[key];
    if (typeof entry === 'string') push[key] = entry;
  }
  return push;
}
