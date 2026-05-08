import { preferences, showToast, notificationsFilter, currentModel, reasoningEffort } from '../store';
import { toFailed } from '../types';
import { getPreferences, setPreference } from '../../api/client';
import { getDeviceId } from './devices';
import { errorDetail } from '../../utils/errorDetail';
import { MODELS, REASONING_LEVELS, clampReasoningEffort, DEFAULT_CHAT_MODEL } from '../models';
import { isIOS } from '../../utils/platform';

export type Theme = 'light' | 'dark' | 'system';
export type FontFamily = 'monospace' | 'system' | 'inter' | 'jetbrains-mono' | 'ibm-plex-mono';
export type ImageModel = 'auto' | 'imagen-4' | 'gpt-image-1' | 'gpt-image-1.5' | 'gpt-image-2';

export const UI_SCALE_MIN = 75;
export const UI_SCALE_MAX = 200;
export const UI_SCALE_STEP = 5;
export const UI_SCALE_DEFAULT = 100;

const FONT_FAMILY_VALUES: Record<FontFamily, string> = {
  monospace: "'SF Mono', 'Fira Code', 'JetBrains Mono', Monaco, Consolas, monospace",
  system: "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
  inter: "'Inter', system-ui, sans-serif",
  'jetbrains-mono': "'JetBrains Mono', monospace",
  'ibm-plex-mono': "'IBM Plex Mono', monospace",
};

const GOOGLE_FONT_URLS: Partial<Record<FontFamily, string>> = {
  inter: 'https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap',
  'jetbrains-mono': 'https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;600;700&display=swap',
  'ibm-plex-mono': 'https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600;700&display=swap',
};

const loadedFonts = new Set<string>();

let systemThemeQuery: MediaQueryList | null = null;
// Seeded so loadPreferences can skip a no-op applyTheme when unchanged.
let lastAppliedTheme: Theme = currentTheme();
// Module-init install: loadPreferences skips applyTheme when the stored
// theme already matches lastAppliedTheme, so without this call a user with
// `system` set would never get the OS-change listener attached.
syncSystemThemeListener(lastAppliedTheme);

// --- Generic helpers ---

function currentPreference<T extends string>(
  key: string,
  validValues: readonly T[],
  defaultValue: T,
  localStorageKey?: string,
): T {
  if (preferences.value.status === 'loaded') {
    const raw = preferences.value.data[key];
    if (raw && (validValues as readonly string[]).includes(raw)) return raw as T;
  }
  if (localStorageKey) {
    const cached = localStorage.getItem(localStorageKey);
    if (cached && (validValues as readonly string[]).includes(cached)) return cached as T;
  }
  return defaultValue;
}

export async function savePreference(
  key: string,
  value: string,
  applySideEffect?: () => void,
  deviceScoped = false,
): Promise<void> {
  applySideEffect?.();
  if (preferences.value.status === 'loaded') {
    preferences.value = {
      status: 'loaded',
      data: { ...preferences.value.data, [key]: value },
    };
  }
  try {
    await setPreference(key, value, deviceScoped ? getDeviceId() : undefined);
  } catch (e) {
    showToast(`Failed to save ${key} preference: ${errorDetail(e)}`, 'error');
  }
}

// --- UI scale ---

export function applyUiScale(scale: number): void {
  const clamped = Math.max(UI_SCALE_MIN, Math.min(UI_SCALE_MAX, scale));
  localStorage.setItem('lucidos-ui-scale', String(clamped));
  document.documentElement.style.setProperty('--user-ui-scale', `${clamped}%`);
}

export function currentUiScale(): number {
  if (preferences.value.status !== 'loaded') return UI_SCALE_DEFAULT;
  const raw = preferences.value.data['ui-scale'] || preferences.value.data['text-size'] || preferences.value.data['font-size'];
  if (!raw) return UI_SCALE_DEFAULT;
  // Migrate old enum values
  const legacyMap: Record<string, number> = { small: 100, medium: 113, large: 125 };
  if (raw in legacyMap) return legacyMap[raw];
  const parsed = parseInt(raw, 10);
  return isNaN(parsed) ? UI_SCALE_DEFAULT : Math.max(UI_SCALE_MIN, Math.min(UI_SCALE_MAX, parsed));
}

export function setUiScale(scale: number): Promise<void> {
  const clamped = Math.max(UI_SCALE_MIN, Math.min(UI_SCALE_MAX, scale));
  return savePreference('ui-scale', String(clamped), () => applyUiScale(clamped), true);
}

// --- Theme ---

function resolveTheme(theme: Theme): 'light' | 'dark' {
  if (theme === 'system') {
    return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
  }
  return theme;
}

export function applyTheme(theme: Theme): void {
  const resolved = resolveTheme(theme);
  const bg = resolved === 'light' ? '#ffffff' : '#0d1117';
  // Theme-flash telemetry — index.html installs __themeLogEvt as a fetch shim
  // that POSTs to /api/internal/client-log (engine.log breadcrumbs).
  type ThemeLogEvt = (label: string, info: unknown) => void;
  const logEvt = (window as unknown as { __themeLogEvt?: ThemeLogEvt }).__themeLogEvt;
  if (logEvt) {
    logEvt('applyTheme', {
      input: theme,
      resolved,
      priorDataTheme: document.documentElement.getAttribute('data-theme'),
      mqLight: window.matchMedia('(prefers-color-scheme: light)').matches,
    });
  }
  localStorage.setItem('lucidos-theme', theme);
  document.documentElement.setAttribute('data-theme', resolved);
  document.documentElement.style.setProperty('--bg-primary', bg);
  // Mirrors the inline FOUC IIFE in index.html — keeps <html> covered on
  // toggle and on the next cold reload, before global.css re-applies its
  // `html { background: var(--bg-primary); }` rule.
  document.documentElement.style.background = bg;

  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) {
    meta.setAttribute('content', bg);
  }

  document.documentElement.style.colorScheme = resolved;

  syncSystemThemeListener(theme);
  lastAppliedTheme = theme;
}

// iOS WKWebView fires prefers-color-scheme change events with wrong values
// at random moments (telemetry-confirmed: 24+ flashes in one session), so the
// listener is skipped there and `system` mode resolves once per page load.
function syncSystemThemeListener(theme: Theme): void {
  if (systemThemeQuery) {
    systemThemeQuery.removeEventListener('change', onSystemThemeChange);
    systemThemeQuery = null;
  }
  if (theme === 'system' && !isIOS()) {
    systemThemeQuery = window.matchMedia('(prefers-color-scheme: light)');
    systemThemeQuery.addEventListener('change', onSystemThemeChange);
  }
}

function onSystemThemeChange(): void {
  applyTheme('system');
}

export function currentTheme(): Theme {
  // localStorage fallback matches the FOUC prevention script in index.html.
  // Covers: backend missing the preference (device_id change, save failure),
  // and the loading window before the API responds.
  return currentPreference('theme', ['light', 'dark', 'system'], 'dark', 'lucidos-theme');
}

export function setTheme(theme: Theme): Promise<void> {
  return savePreference('theme', theme, () => applyTheme(theme), true);
}

// --- Font family ---

function ensureFontLoaded(font: FontFamily): void {
  const url = GOOGLE_FONT_URLS[font];
  if (!url || loadedFonts.has(font)) return;
  loadedFonts.add(font);
  const link = document.createElement('link');
  link.rel = 'stylesheet';
  link.href = url;
  document.head.appendChild(link);
}

export function applyFontFamily(font: FontFamily): void {
  ensureFontLoaded(font);
  localStorage.setItem('lucidos-font-family', font);
  const value = FONT_FAMILY_VALUES[font] || FONT_FAMILY_VALUES.monospace;
  document.documentElement.style.setProperty('--font-ui', value);
}

export function currentFontFamily(): FontFamily {
  return currentPreference('font-family', Object.keys(FONT_FAMILY_VALUES) as FontFamily[], 'monospace');
}

export function setFontFamily(font: FontFamily): Promise<void> {
  return savePreference('font-family', font, () => applyFontFamily(font), true);
}

// --- Image model ---

export function currentImageModel(): ImageModel {
  return currentPreference('image_model', ['auto', 'imagen-4', 'gpt-image-1', 'gpt-image-1.5', 'gpt-image-2'], 'auto');
}

export function setImageModel(model: ImageModel): Promise<void> {
  return savePreference('image_model', model);
}

// --- Notifications filter ---

export function currentNotificationsFilter(): 'all' | 'unread' {
  return currentPreference('notifications_filter', ['all', 'unread'], 'all', 'lucidos-notifications-filter');
}

// --- Chat model & reasoning effort ---

const MODEL_VALUES = MODELS.map(m => m.value);
const REASONING_VALUES = REASONING_LEVELS.map(l => l.value);

export function currentChatModel(): string {
  return currentPreference('chat_model', MODEL_VALUES, DEFAULT_CHAT_MODEL);
}

export function currentChatReasoningEffort(): string {
  return currentPreference('chat_reasoning_effort', REASONING_VALUES, 'high');
}

export async function setCurrentModel(model: string): Promise<void> {
  const oldEffort = reasoningEffort.value;
  const clamped = clampReasoningEffort(oldEffort, model);
  await savePreference('chat_model', model, () => { currentModel.value = model; });
  if (clamped !== oldEffort) {
    await setReasoningEffort(clamped);
  }
}

export function setReasoningEffort(effort: string): Promise<void> {
  return savePreference('chat_reasoning_effort', effort, () => {
    reasoningEffort.value = effort;
  });
}

// --- Load all preferences ---

export async function loadPreferences(): Promise<void> {
  // Only flip to 'loading' on the first fetch — refetches (e.g. after an SSE
  // PreferencesChanged) keep showing existing data through the network round
  // trip and swap atomically when the response lands. Without this guard,
  // every preference toggle wipes subscribers to defaults until the GET
  // completes, which the user sees as a flash.
  if (preferences.value.status === 'not-loaded') {
    preferences.value = { status: 'loading' };
  }
  try {
    const res = await getPreferences(getDeviceId());
    preferences.value = { status: 'loaded', data: res.preferences };
    applyUiScale(currentUiScale());
    const t = currentTheme();
    if (t !== lastAppliedTheme) applyTheme(t);
    applyFontFamily(currentFontFamily());
    currentModel.value = currentChatModel();
    reasoningEffort.value = clampReasoningEffort(currentChatReasoningEffort(), currentModel.value);
    notificationsFilter.value = currentNotificationsFilter();
  } catch (e) {
    preferences.value = toFailed(e);
  }
}

// --- Vertex AI region ---

export const DEFAULT_VERTEX_REGION = 'europe-west1';

export function currentVertexRegion(): string {
  if (preferences.value.status !== 'loaded') return DEFAULT_VERTEX_REGION;
  return preferences.value.data['vertex_region'] || DEFAULT_VERTEX_REGION;
}

export function setVertexRegion(region: string): Promise<void> {
  return savePreference('vertex_region', region);
}

// --- Background model ---

/** Background model preference keys — stored in the DB, read by the engine. */
export type BackgroundModelKey = 'model_title' | 'model_image_description' | 'model_memory';

export function currentBackgroundModel(key: BackgroundModelKey): string {
  if (preferences.value.status !== 'loaded') return 'gemini-3-flash-preview';
  return preferences.value.data[key] || 'gemini-3-flash-preview';
}

export function setBackgroundModel(key: BackgroundModelKey, model: string): Promise<void> {
  return savePreference(key, model);
}
