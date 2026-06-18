import { preferences, showToast, notificationsFilter, currentModel, reasoningEffort, selectedCodingAgent } from '../store';
import type { CodingAgent } from '../../api/types';
import { toFailed } from '../types';
import { getPreferences, setPreference } from '../../api/client';
import { getDeviceId } from './devices';
import { errorDetail } from '../../utils/errorDetail';
import { REASONING_LEVELS, clampReasoningEffort, DEFAULT_CHAT_MODEL } from '../models';
import { isIOS } from '../../utils/platform';

export type Theme = 'light' | 'dark' | 'system';
export type FontFamily = 'monospace' | 'system' | 'inter' | 'jetbrains-mono' | 'ibm-plex-mono';
export type ImageModel = 'auto' | 'imagen-4' | 'gpt-image-1' | 'gpt-image-1.5' | 'gpt-image-2';

export const UI_SCALE_MIN = 75;
export const UI_SCALE_MAX = 200;
// 12.5% keeps the root font-size on integer pixels (16 × 0.125 = 2px per
// step), so every `rem` resolves to an integer and 1px borders don't
// anti-alias at varying widths across the layout. Inline duplicates exist
// in FOUC scripts — grep for `Math.round(n / 12.5) * 12.5`.
export const UI_SCALE_STEP = 12.5;
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

export function clampUiScale(scale: number): number {
  const snapped = Math.round(scale / UI_SCALE_STEP) * UI_SCALE_STEP;
  return Math.max(UI_SCALE_MIN, Math.min(UI_SCALE_MAX, snapped));
}

export function applyUiScale(scale: number): void {
  const clamped = clampUiScale(scale);
  localStorage.setItem('lucidos-ui-scale', String(clamped));
  document.documentElement.style.setProperty('--user-ui-scale', `${clamped}%`);
}

export function currentUiScale(): number {
  if (preferences.value.status !== 'loaded') return UI_SCALE_DEFAULT;
  const raw = preferences.value.data['ui-scale'] || preferences.value.data['text-size'] || preferences.value.data['font-size'];
  if (!raw) return UI_SCALE_DEFAULT;
  // Migrate old enum values. `medium` was 113 (pre-12.5%-grid); snap to 112.5.
  const legacyMap: Record<string, number> = { small: 100, medium: 112.5, large: 125 };
  if (raw in legacyMap) return legacyMap[raw];
  // parseFloat so fractional snapped values like "112.5" round-trip.
  const parsed = parseFloat(raw);
  return isNaN(parsed) ? UI_SCALE_DEFAULT : clampUiScale(parsed);
}

export function setUiScale(scale: number): Promise<void> {
  const clamped = clampUiScale(scale);
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
  const bg = resolved === 'light' ? '#ffffff' : '#0b2342';
  // Theme-flash telemetry — index.html installs __themeLogEvt as a fetch shim
  // that POSTs to /api/v1/internal/client-log (engine.log breadcrumbs).
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

const REASONING_VALUES = REASONING_LEVELS.map(l => l.value);

/** The user's chat model preference. Unlike most preferences this is NOT
 *  validated against a fixed allow-list — the model set is now the DB-backed
 *  registry (user-extensible), and `RoutingProvider` resolves any id (with a
 *  prefix fallback), so any stored non-empty value is honored. */
export function currentChatModel(): string {
  if (preferences.value.status !== 'loaded') return DEFAULT_CHAT_MODEL;
  const v = preferences.value.data['chat_model'];
  return v && v.trim() ? v : DEFAULT_CHAT_MODEL;
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
    selectedCodingAgent.value = currentCodingAgentDefault();
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

// --- Local OpenAI-compatible provider base URL ---

// Ollama's OpenAI-compatible endpoint. Mirrors DEFAULT_LOCAL_BASE_URL in
// crates/lucidos-engine/src/core/preferences.rs.
export const DEFAULT_LOCAL_BASE_URL = 'http://localhost:11434/v1';

export function currentLocalBaseUrl(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['local_base_url'] || '';
}

export function setLocalBaseUrl(url: string): Promise<void> {
  return savePreference('local_base_url', url.trim());
}

// --- Capture context ---

/** Per-step ContextAssembled capture toggle. Defaults to true (on). */
export function currentCaptureContext(): boolean {
  if (preferences.value.status !== 'loaded') return true;
  const raw = preferences.value.data['capture_context'];
  if (raw == null) return true;
  return raw !== 'false';
}

export function setCaptureContext(enabled: boolean): Promise<void> {
  return savePreference('capture_context', enabled ? 'true' : 'false');
}

// --- Mobile header sticky ---

/** When true, the mobile header stays fully visible — disables hide-on-scroll,
 *  hide-on-keyboard-open, and the app-UI-active pin. Defaults to true; only an
 *  explicit `'false'` opts out of the pinned header. */
export function currentMobileHeaderSticky(): boolean {
  if (preferences.value.status !== 'loaded') return true;
  return preferences.value.data['mobile_header_sticky'] !== 'false';
}

export function setMobileHeaderSticky(enabled: boolean): Promise<void> {
  return savePreference('mobile_header_sticky', enabled ? 'true' : 'false');
}

// --- Background model ---

/** Background model preference keys — stored in the DB, read by the engine. */
export type BackgroundModelKey =
  | 'model_title'
  | 'model_image_description'
  | 'model_memory'
  | 'model_command_judge';

/** Default model for the command-guard judge (Haiku, per ADR 0002). Mirrors the
 *  backend `DEFAULT_COMMAND_JUDGE_MODEL` in `core/preferences.rs`. */
export const DEFAULT_COMMAND_JUDGE_MODEL = 'claude-haiku-4-5';

/** Per-key default shown when the preference is unset — most background tasks
 *  default to Gemini Flash; the command-guard judge defaults to Haiku. */
const BACKGROUND_MODEL_DEFAULTS: Record<BackgroundModelKey, string> = {
  model_title: 'gemini-3-flash-preview',
  model_image_description: 'gemini-3-flash-preview',
  model_memory: 'gemini-3-flash-preview',
  model_command_judge: DEFAULT_COMMAND_JUDGE_MODEL,
};

export function currentBackgroundModel(key: BackgroundModelKey): string {
  const fallback = BACKGROUND_MODEL_DEFAULTS[key];
  if (preferences.value.status !== 'loaded') return fallback;
  return preferences.value.data[key] || fallback;
}

export function setBackgroundModel(key: BackgroundModelKey, model: string): Promise<void> {
  return savePreference(key, model);
}

// --- Compose destination (coding-agent chip + hand-off hint) ---

/** Last-used coding agent for the compose destination picker's chip.
 *  Workspace-scoped (not device-scoped) so the pick follows the user across
 *  devices. Default Claude Code — same as the engine's NULL fallback. */
export function currentCodingAgentDefault(): CodingAgent {
  return currentPreference('coding_agent_default', ['claude-code', 'codex'], 'claude-code');
}

export function setCodingAgentDefault(agent: CodingAgent): Promise<void> {
  // The Dropdown fires onChange on every click, including the already-selected
  // option — skip the no-op so a re-pick doesn't PUT + fan out
  // PreferencesChanged for an unchanged value.
  if (selectedCodingAgent.value === agent) return Promise.resolve();
  return savePreference('coding_agent_default', agent, () => {
    selectedCodingAgent.value = agent;
  });
}

/** Whether the compose view's "Not sure? Start with the Lucidos Agent" hint
 *  has been retired. Treats not-yet-loaded (and failed) preferences as
 *  dismissed so a user who already retired it never sees a flash during the
 *  preferences fetch — an onboarding hint fails closed. */
export function composeHandoffHintDismissed(): boolean {
  if (preferences.value.status !== 'loaded') return true;
  return preferences.value.data['compose_handoff_hint_dismissed'] === 'true';
}

/** One-way retire — called on the user's first send to a coding destination
 *  (`sendCompose`) and on explicit ✕ dismiss. Idempotent: skips the write
 *  only when the LOADED preference already says dismissed — while preferences
 *  are still loading the write goes through, so a fast first coding send
 *  can't slip past the retire. */
export function dismissComposeHandoffHint(): Promise<void> {
  if (preferences.value.status === 'loaded'
    && preferences.value.data['compose_handoff_hint_dismissed'] === 'true') {
    return Promise.resolve();
  }
  return savePreference('compose_handoff_hint_dismissed', 'true');
}

// --- New-workspace welcome + starter suggestions ---

/** Whether the new-workspace welcome + starter suggestions have been retired
 *  via "Don't show this again". Treats not-yet-loaded (and failed) preferences
 *  as dismissed so a returning user who already retired it never sees a flash
 *  during the preferences fetch — an onboarding surface fails closed. A genuinely
 *  new workspace gets it back once the preference loads and reads unset. */
export function welcomeSuggestionsDismissed(): boolean {
  if (preferences.value.status !== 'loaded') return true;
  return preferences.value.data['welcome_suggestions_dismissed'] === 'true';
}

/** One-way retire — the user clicked "Don't show this again" on the
 *  new-workspace welcome. Idempotent: skips the write only when the LOADED
 *  preference already says dismissed. */
export function dismissWelcomeSuggestions(): Promise<void> {
  if (preferences.value.status === 'loaded'
    && preferences.value.data['welcome_suggestions_dismissed'] === 'true') {
    return Promise.resolve();
  }
  return savePreference('welcome_suggestions_dismissed', 'true');
}

// --- Command guard (ADR 0002) ---

/** Master toggle for the command guard (the bash/python safety gate). Defaults
 *  to false — the feature ships dark and is enabled per-workspace. Mirrors the
 *  backend `command_guard` preference (`core/preferences.rs`). */
export function currentCommandGuard(): boolean {
  if (preferences.value.status !== 'loaded') return false;
  return preferences.value.data['command_guard'] === 'true';
}

export function setCommandGuard(enabled: boolean): Promise<void> {
  return savePreference('command_guard', enabled ? 'true' : 'false');
}

/** Sub-toggle for the LLM judge — when off, the guard uses only the static
 *  "dangerous" list for the ask lane. Defaults to true (on when the guard is on).
 *  Only meaningful while the master `command_guard` toggle is on. */
export function currentCommandGuardJudge(): boolean {
  if (preferences.value.status !== 'loaded') return true;
  return preferences.value.data['command_guard_judge'] !== 'false';
}

export function setCommandGuardJudge(enabled: boolean): Promise<void> {
  return savePreference('command_guard_judge', enabled ? 'true' : 'false');
}
