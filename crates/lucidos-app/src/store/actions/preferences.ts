import { PREF_AGENTS_UNSANDBOXED, PREF_AGENT_ICON_COLORS, PREF_AUTOCORRECT, PREF_CACHE_CARDS, PREF_CACHE_KEEP_ALIVE_MODE, PREF_CAPTURE_CONTEXT, PREF_CHATGPT_PLAN_WELCOME_SHOWN, PREF_CHAT_MODEL, PREF_CHAT_REASONING_EFFORTS, PREF_CODING_AGENT_AUTO_ANSWER, PREF_CODING_AGENT_CLAUDE_PERMISSION_MODE, PREF_CODING_AGENT_CONTEXT_CEILING, PREF_CODING_AGENT_DEFAULT, PREF_COMMAND_GUARD, PREF_COMMAND_GUARD_JUDGE, PREF_COMPOSER_ENTER_KEY, PREF_EXTERNAL_LINK_TARGET, PREF_IMAGE_MODEL, PREF_LOCAL_BASE_URL, PREF_MAIN_CHECK_SETTING, PREF_MAX_TOOL_CALLS, PREF_MEMORY_MODULE, PREF_MOBILE_DYNAMIC_BARS, PREF_MODEL_CHANGE_SUMMARY, PREF_MODEL_COMMAND_JUDGE, PREF_MODEL_CONVERSATION_SUMMARY, PREF_MODEL_EXPECTED_WAKE_DECISION, PREF_MODEL_IMAGE_DESCRIPTION, PREF_MODEL_MEMORY, PREF_MODEL_PERMISSION_ANSWER, PREF_MODEL_QUERY_CLASSIFICATION, PREF_MODEL_READ_DECISION, PREF_MODEL_SUMMARY_COMPACTION, PREF_MODEL_TITLE, PREF_MODEL_VOICE_TALKER, PREF_MODEL_VOICE_TRANSCRIBER, PREF_MODEL_WAKE_DECISION, PREF_MOTION, PREF_NOTIFICATIONS_FILTER, PREF_NOTIFICATION_TOASTS, PREF_OPENCODE_FREE_ENABLED, PREF_REASONING_CHANGE_SUMMARY, PREF_REASONING_COMMAND_JUDGE, PREF_REASONING_CONVERSATION_SUMMARY, PREF_REASONING_EXPECTED_WAKE_DECISION, PREF_REASONING_IMAGE_DESCRIPTION, PREF_REASONING_MEMORY, PREF_REASONING_PERMISSION_ANSWER, PREF_REASONING_QUERY_CLASSIFICATION, PREF_REASONING_READ_DECISION, PREF_REASONING_SUMMARY_COMPACTION, PREF_REASONING_TITLE, PREF_REASONING_WAKE_DECISION, PREF_RESPONSE_STYLE, PREF_TECHNICAL_LITERACY, PREF_TERMINAL_ENABLED, PREF_THEME_EFFECTS, PREF_THEME_MODE, PREF_TRIGGER_MODEL, PREF_VERTEX_REGION, PREF_VOICE_ENABLED, PREF_VOICE_RESIDENT_SECTIONS, PREF_VOICE_TALKER_VOICE } from '@lucidos/preference-catalog';
import { applyCodingAgentDefault } from '../composeSelections';
import { computed, effect, signal, untracked } from '@preact/signals';
import { clampDashboardRailWidth } from '../dashboardRail';
import { preferences, showToast, removeToast, notificationsFilter, NOTIFICATIONS_FILTER_STORAGE_KEY, currentModel, selectedCodingAgent, clampThreadDrawerWidth, llmConfigured, welcomeDismissed, lastSuccessfulBackup, type NotificationsFilter } from '../store';
import type { CodingAgent } from '../../api/types';
import type { ThreadState } from '../thread-events';
import { failedIfFresh, type Loadable } from '../types';
import { startSliceLoad, type SliceOf } from '../bootstrapSliceLoads';
import { getPreferences, setPreference, isTransientFetchError, retryTransient, getTheme, ApiError, type LandingSetting, type LastSuccessfulBackupResponse, type MainCheckSetting } from '../../api/client';
import { resolvedTokenHex } from '../../utils/cssColor';
import { getDeviceId } from './devices';
import { errorDetail } from '../../utils/errorDetail';
import { createFailureCounter } from '../../utils/failureCounter';
import { clampEffortFor, modelDefaultEffort } from './models';
import { parseChatEfforts, withChatEffort } from '../chatEfforts';
import { isIOSPwa, isTauri } from '../../utils/platform';
import { setProseAutocorrect } from '../../utils/noAutofill';
import { publishScrollbarGutter } from '../../utils/scrollbarGutter';
import { setTitlebarColor, windowReadyToShow } from '../../utils/tauri';
import {
  STYLE_OVERRIDES_KEY, STYLE_OVERRIDES_STORAGE_KEY, STYLE_RESET_PARAM,
  isAllowedOverride, parseStyleOverrides,
  serializeStyleOverrides, styleResetRequested,
} from '../../utils/styleOverrides';

import {
  EMPTY_THEME, FONT_PREFERENCES, THEME_EFFECTS_STORAGE_KEY,
  THEME_KEY, THEME_STORAGE_KEY,
  MOTION_STORAGE_KEY, THEME_MODE_ATTRIBUTE, THEME_MODE_BG, THEME_MODE_HEADER_TOP,
  THEME_MODE_KEY, THEME_MODE_STORAGE_KEY, UI_SCALE_STORAGE_KEY,
  FONT_BOLD_ATTRIBUTE, FONT_FAMILY_STORAGE_KEY, WORKSPACE_FONT_STORAGE_KEY,
  clampUiScale, followSystemThemeMode, fontBoldMark, isWorkspaceFontId, themeBackground, themeTokenNames, parseResolvedTheme,
  parseUiScale, parseWorkspaceFont, repaintThemeMode, replaceInlineTokens, resolveFont, resolveThemeMode,
  sanitizeResolvedTheme,
  type FontPreference, type ThemeEffectsPref, type MotionPref, type ResolvedTheme,
  type ResolvedThemeMode, type ThemeMode, type WorkspaceFont,
} from '@lucidos/appearance';
import { registerFontsInUse } from '@lucidos/font-faces';
import { dataMountUrl } from '../../api/client';
import { loadWorkspaceFonts, workspaceFontList } from './workspaceFonts';
import { motionPreference } from '../../utils/motion';
import { themeEffectsPreference } from '../../utils/themeEffects';
import { AUTOCORRECT_STORAGE_KEY } from '@lucidos/text-entry';
import {
  parseFlag, type PREFERENCE_CATALOG, PREF_FONT_FAMILY, PREF_PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI,
  PREF_PROVIDER_ENABLED_OPENAI, PREF_PROVIDER_ENABLED_OPENROUTER,
  PREF_PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM, PREF_PROVIDER_ENABLED_TYPESAFE, PREF_SYSTEM_ONE_CUSTOM_MODEL,
  PREF_SYSTEM_ONE_CUSTOM_URL, PREF_SYSTEM_ONE_OPENROUTER_MODEL, PREF_THEME, PREF_TITLE_STYLE, PREF_UI_SCALE,
  type PreferenceKey, type PreferenceValues,
} from '@lucidos/preference-catalog';
import { MEMORY_MODULE_WHILE_UNSET, VOICE_RESIDENT_SECTIONS } from '@lucidos/engine-constants';

export type { FontId, FontPreference, ThemeMode } from '@lucidos/appearance';
export {
  UI_SCALE_MIN, UI_SCALE_MAX, UI_SCALE_STEP, clampUiScale,
} from '@lucidos/appearance';

export const UI_SCALE_DEFAULT = Number(PREF_UI_SCALE.fallback);

export const DEFAULT_THEME_ID = PREF_THEME.fallback;

const DEFAULT_FONT_PREFERENCE: FontPreference = PREF_FONT_FAMILY.fallback;

export type ImageModel = PreferenceValues<'image_model'>;


const READ_PREFERENCES = {
  'agent-icon-colors': PREF_AGENT_ICON_COLORS,
  'agents_unsandboxed': PREF_AGENTS_UNSANDBOXED,
  'autocorrect': PREF_AUTOCORRECT,
  'cache_cards': PREF_CACHE_CARDS,
  'cache_keep_alive_mode': PREF_CACHE_KEEP_ALIVE_MODE,
  'capture_context': PREF_CAPTURE_CONTEXT,
  'terminal_enabled': PREF_TERMINAL_ENABLED,
  'chat_model': PREF_CHAT_MODEL,
  'chat_reasoning_efforts': PREF_CHAT_REASONING_EFFORTS,
  'chatgpt_plan_welcome_shown': PREF_CHATGPT_PLAN_WELCOME_SHOWN,
  'coding_agent_auto_answer': PREF_CODING_AGENT_AUTO_ANSWER,
  'coding_agent_claude_permission_mode': PREF_CODING_AGENT_CLAUDE_PERMISSION_MODE,
  'coding_agent_context_ceiling': PREF_CODING_AGENT_CONTEXT_CEILING,
  'coding_agent_default': PREF_CODING_AGENT_DEFAULT,
  'command_guard': PREF_COMMAND_GUARD,
  'command_guard_judge': PREF_COMMAND_GUARD_JUDGE,
  'composer_enter_key': PREF_COMPOSER_ENTER_KEY,
  'external_link_target': PREF_EXTERNAL_LINK_TARGET,
  'image_model': PREF_IMAGE_MODEL,
  'main_check_setting': PREF_MAIN_CHECK_SETTING,
  'memory_module': PREF_MEMORY_MODULE,
  'mobile_dynamic_bars': PREF_MOBILE_DYNAMIC_BARS,
  'model_change_summary': PREF_MODEL_CHANGE_SUMMARY,
  'model_command_judge': PREF_MODEL_COMMAND_JUDGE,
  'model_conversation_summary': PREF_MODEL_CONVERSATION_SUMMARY,
  'model_expected_wake_decision': PREF_MODEL_EXPECTED_WAKE_DECISION,
  'model_image_description': PREF_MODEL_IMAGE_DESCRIPTION,
  'model_memory': PREF_MODEL_MEMORY,
  'model_permission_answer': PREF_MODEL_PERMISSION_ANSWER,
  'model_query_classification': PREF_MODEL_QUERY_CLASSIFICATION,
  'model_read_decision': PREF_MODEL_READ_DECISION,
  'model_summary_compaction': PREF_MODEL_SUMMARY_COMPACTION,
  'model_title': PREF_MODEL_TITLE,
  'model_voice_talker': PREF_MODEL_VOICE_TALKER,
  'model_voice_transcriber': PREF_MODEL_VOICE_TRANSCRIBER,
  'model_wake_decision': PREF_MODEL_WAKE_DECISION,
  'motion': PREF_MOTION,
  'notification_toasts': PREF_NOTIFICATION_TOASTS,
  'notifications_filter': PREF_NOTIFICATIONS_FILTER,
  'opencode_free_enabled': PREF_OPENCODE_FREE_ENABLED,
  'reasoning_change_summary': PREF_REASONING_CHANGE_SUMMARY,
  'reasoning_command_judge': PREF_REASONING_COMMAND_JUDGE,
  'reasoning_conversation_summary': PREF_REASONING_CONVERSATION_SUMMARY,
  'reasoning_expected_wake_decision': PREF_REASONING_EXPECTED_WAKE_DECISION,
  'reasoning_image_description': PREF_REASONING_IMAGE_DESCRIPTION,
  'reasoning_memory': PREF_REASONING_MEMORY,
  'reasoning_permission_answer': PREF_REASONING_PERMISSION_ANSWER,
  'reasoning_query_classification': PREF_REASONING_QUERY_CLASSIFICATION,
  'reasoning_read_decision': PREF_REASONING_READ_DECISION,
  'reasoning_summary_compaction': PREF_REASONING_SUMMARY_COMPACTION,
  'reasoning_title': PREF_REASONING_TITLE,
  'reasoning_wake_decision': PREF_REASONING_WAKE_DECISION,
  'response_style': PREF_RESPONSE_STYLE,
  'theme-effects': PREF_THEME_EFFECTS,
  'theme-mode': PREF_THEME_MODE,
  'trigger_model': PREF_TRIGGER_MODEL,
  'vertex_region': PREF_VERTEX_REGION,
  'voice_enabled': PREF_VOICE_ENABLED,
  'voice_talker_voice': PREF_VOICE_TALKER_VOICE,
} as const;
type ReadPreferenceKey = keyof typeof READ_PREFERENCES;

let systemThemeModeQuery: MediaQueryList | null = null;
let lastAppliedThemeMode: ThemeMode = currentThemeMode();
export const paintedThemeMode = signal<ResolvedThemeMode>(
  document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE) === 'light' ? 'light' : 'dark',
);
export const appearanceVersion = signal(0);


interface CatalogEntry {
  readonly type: string;
  readonly values?: readonly string[];
  readonly inherits?: PreferenceKey;
  readonly fallback: string | null;
}

type ResolvableKey = {
  [K in PreferenceKey]: (typeof PREFERENCE_CATALOG)[K] extends {
    type: 'enum' | 'flag' | 'text';
    fallback: string;
  } ? K : never;
}[PreferenceKey] & ReadPreferenceKey;

type ResolvedPreference<K extends ResolvableKey> =
  (typeof PREFERENCE_CATALOG)[K] extends { values: readonly (infer V)[] } ? V
    : (typeof PREFERENCE_CATALOG)[K] extends { type: 'flag' } ? 'true' | 'false'
      : string;

function flagReading(raw: string): 'true' | 'false' | null {
  const on = parseFlag(raw);
  return on === null ? null : on ? 'true' : 'false';
}

function acceptedValue(key: ReadPreferenceKey, raw: string | null | undefined): string | null {
  if (!raw) return null;
  const entry: CatalogEntry = READ_PREFERENCES[key];
  if (entry.values) return entry.values.includes(raw) ? raw : null;
  if (entry.type === 'flag') return flagReading(raw);
  const text = raw.trim();
  return text === '' ? null : text;
}

function currentPreference<K extends ResolvableKey>(key: K, localStorageKey?: string): ResolvedPreference<K> {
  if (preferences.value.status === 'loaded') {
    const stored = acceptedValue(key, preferences.value.data[key]);
    if (stored !== null) return stored as ResolvedPreference<K>;
  }
  if (localStorageKey) {
    const cached = acceptedValue(key, localStorage.getItem(localStorageKey));
    if (cached !== null) return cached as ResolvedPreference<K>;
  }
  const entry: CatalogEntry = READ_PREFERENCES[key];
  if (entry.inherits) return currentPreference(entry.inherits as ResolvableKey) as ResolvedPreference<K>;
  return entry.fallback as ResolvedPreference<K>;
}

function cacheServedValue(key: ResolvableKey, storageKey: string): void {
  const served = acceptedValue(key, preferences.value.status === 'loaded' ? preferences.value.data[key] : undefined);
  if (served !== null) {
    localStorage.setItem(storageKey, served);
  } else {
    localStorage.removeItem(storageKey);
  }
}


interface PendingPreferenceWrite {
  value: string;
  deviceScoped: boolean;
  seq: number;
}

const pendingPreferenceWrites = new Map<string, PendingPreferenceWrite>();
let writeSeq = 0;

const latestWriteSeq = new Map<string, number>();

const unansweredWrites = new Map<string, PendingPreferenceWrite>();

const refusedWriteSeq = new Map<string, number>();

function writeValues(writes: Iterable<[string, PendingPreferenceWrite]>): Record<string, string> {
  return Object.fromEntries([...writes].map(([key, write]) => [key, write.value]));
}

const deliveryChains = new Map<string, Promise<unknown>>();

function enqueueDelivery<T>(key: string, run: () => Promise<T>): Promise<T> {
  const prior = deliveryChains.get(key) ?? Promise.resolve();
  const next = prior.then(run, run);
  deliveryChains.set(key, next);
  void next.finally(() => {
    if (deliveryChains.get(key) === next) deliveryChains.delete(key);
  });
  return next;
}

const PREFERENCE_REJECTED_TOAST = 'preference-save-rejected';
const PREFERENCE_UNREACHABLE_TOAST = 'preference-save-unreachable';

const writeFailures = createFailureCounter(3, () => {
  const stuck = [...pendingPreferenceWrites.keys()].sort().join(', ');
  showToast(
    `Preference changes are not reaching the engine (${stuck}). They are applied on this device and will be re-sent automatically.`,
    'error',
    { key: PREFERENCE_UNREACHABLE_TOAST },
  );
});

function settleDelivered(key: string, write: PendingPreferenceWrite): void {
  pendingPreferenceWrites.delete(key);
  if (unansweredWrites.get(key)?.seq === write.seq) unansweredWrites.delete(key);
  writeFailures.recordSuccess();
  if (pendingPreferenceWrites.size === 0) removeToast(PREFERENCE_UNREACHABLE_TOAST);
}

function deliverOrPark(key: string, write: PendingPreferenceWrite): Promise<boolean> {
  return enqueueDelivery(key, () => deliverNow(key, write));
}

async function deliverNow(key: string, write: PendingPreferenceWrite): Promise<boolean> {
  for (let attempt = 0; attempt < 2; attempt++) {
    if ((latestWriteSeq.get(key) ?? write.seq) > write.seq) return false;
    try {
      await setPreference(key, write.value, write.deviceScoped ? getDeviceId() : undefined);
    } catch (e) {
      if (isTransientFetchError(e)) continue;
      settleDelivered(key, write);
      refusedWriteSeq.set(key, write.seq);
      showToast(`Failed to save ${key} preference: ${errorDetail(e)}`, 'error', {
        key: PREFERENCE_REJECTED_TOAST,
      });
      return false;
    }
    settleDelivered(key, write);
    return true;
  }
  pendingPreferenceWrites.set(key, write);
  writeFailures.recordFailure();
  return false;
}

export async function flushPendingPreferenceWrites(): Promise<void> {
  if (pendingPreferenceWrites.size === 0) return;
  const entries = [...pendingPreferenceWrites.entries()];
  await Promise.all(entries.map(([key, write]) => deliverOrPark(key, write)));
}

export function _pendingPreferenceKeysForTesting(): string[] {
  return [...pendingPreferenceWrites.keys()].sort();
}

export function _resetPendingPreferenceWritesForTesting(): void {
  pendingPreferenceWrites.clear();
  latestWriteSeq.clear();
  unansweredWrites.clear();
  refusedWriteSeq.clear();
  deliveryChains.clear();
  writeFailures.recordSuccess();
}

export async function savePreference(
  key: string,
  value: string,
  applySideEffect?: () => void,
  deviceScoped = false,
): Promise<void> {
  await savePreferenceAccepted(key, value, applySideEffect, deviceScoped);
}

export async function savePreferenceAccepted(
  key: string,
  value: string,
  applySideEffect?: () => void,
  deviceScoped = false,
): Promise<boolean> {
  applySideEffect?.();
  if (preferences.value.status === 'loaded') {
    preferences.value = {
      status: 'loaded',
      data: { ...preferences.value.data, [key]: value },
    };
  }
  const write = { value, deviceScoped, seq: ++writeSeq };
  latestWriteSeq.set(key, write.seq);
  unansweredWrites.set(key, write);
  return deliverOrPark(key, write);
}


let scaleMeasurementPending = false;

function measureScaleDerivedValues(): void {
  scaleMeasurementPending = false;
  publishScrollbarGutter();
  clampThreadDrawerWidth();
  clampDashboardRailWidth();
}

function scheduleScaleMeasurements(): void {
  if (scaleMeasurementPending) return;
  if (typeof requestAnimationFrame !== 'function') {
    measureScaleDerivedValues();
    return;
  }
  scaleMeasurementPending = true;
  requestAnimationFrame(measureScaleDerivedValues);
}

export function applyUiScale(scale: number): void {
  const clamped = clampUiScale(scale);
  localStorage.setItem(UI_SCALE_STORAGE_KEY, String(clamped));
  document.documentElement.style.setProperty('--user-ui-scale', `${clamped}%`);
  reapplyStyleOverrides();
  scheduleScaleMeasurements();
  appearanceVersion.value++;
}

export function currentUiScale(): number {
  if (preferences.value.status !== 'loaded') return UI_SCALE_DEFAULT;
  const raw = preferences.value.data['ui-scale'] || preferences.value.data['text-size'] || preferences.value.data['font-size'];
  return parseUiScale(raw) ?? UI_SCALE_DEFAULT;
}

let previewedUiScale: number | null = null;

export function previewUiScale(scale: number): void {
  previewedUiScale = clampUiScale(scale);
  applyUiScale(previewedUiScale);
}

export function cancelUiScalePreview(): void {
  const previewed = previewedUiScale;
  previewedUiScale = null;
  if (previewed !== null && previewed !== currentUiScale()) applyUiScale(currentUiScale());
}

export function setUiScale(scale: number): Promise<void> {
  const clamped = clampUiScale(scale);
  previewedUiScale = null;
  return savePreference('ui-scale', String(clamped), () => applyUiScale(clamped), true);
}


function osPrefersLight(): boolean {
  return window.matchMedia('(prefers-color-scheme: light)').matches;
}

const THEME_SWAP_ATTRIBUTE = 'data-theme-swap';
let themeSwapCount = 0;

function markThemeSwap(): void {
  if (typeof requestAnimationFrame !== 'function') return;
  const root = document.documentElement;
  const swap = ++themeSwapCount;
  root.setAttribute(THEME_SWAP_ATTRIBUTE, '');
  requestAnimationFrame(() => requestAnimationFrame(() => {
    if (swap === themeSwapCount) root.removeAttribute(THEME_SWAP_ATTRIBUTE);
  }));
}

export function applyThemeMode(mode: ThemeMode): void {
  let sampledLight: boolean | null = null;
  const resolved = repaintThemeMode(
    mode,
    lastAppliedThemeMode,
    document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE),
    () => (sampledLight = osPrefersLight()),
  );
  paintThemeMode(mode, resolved, sampledLight);
}

function paintThemeMode(mode: ThemeMode, resolved: ResolvedThemeMode, sampledLight: boolean | null): void {
  markThemeSwap();
  const themeTokens = activeTheme[resolved];
  clearStaleThemeTokens(themeTokens);
  paintedThemeMode.value = resolved;
  const bg = themeBackground(themeTokens) ?? THEME_MODE_BG[resolved];
  type ThemeModeLogEvt = (label: string, info: unknown) => void;
  const logEvt = (window as unknown as { __themeModeLogEvt?: ThemeModeLogEvt }).__themeModeLogEvt;
  if (logEvt) {
    logEvt('applyThemeMode', {
      input: mode,
      resolved,
      priorDataThemeMode: document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE),
      mqLight: sampledLight,
    });
  }
  localStorage.setItem(THEME_MODE_STORAGE_KEY, mode);
  document.documentElement.setAttribute(THEME_MODE_ATTRIBUTE, resolved);
  document.documentElement.style.setProperty('--bg-primary', bg);
  document.documentElement.style.background = bg;

  setThemeTokens(themeTokens);
  if (themeTokens['--bg-primary'] && !themeBackground(themeTokens)) {
    document.documentElement.style.background = 'var(--bg-primary)';
  }

  document.documentElement.style.colorScheme = resolved;

  if (isTauri()) {
    const titlebar = resolvedTokenHex('--titlebar-strip-bg') ?? THEME_MODE_HEADER_TOP[resolved];
    setTitlebarColor(titlebar).catch((e) => console.warn('[titlebar] tint failed', e));
    windowReadyToShow();
  }

  syncSystemThemeModeListener(mode);
  lastAppliedThemeMode = mode;
  reapplyStyleOverrides();
  paintThemeColor();
  appearanceVersion.value++;
}

export function paintThemeColor(): void {
  document.querySelector('meta[name="theme-color"]')?.setAttribute(
    'content',
    resolvedTokenHex('--header-bar-top') ?? THEME_MODE_HEADER_TOP[paintedThemeMode.value],
  );
}


let activeTheme: ResolvedTheme = parseResolvedTheme(localStorage.getItem(THEME_STORAGE_KEY));
let requestedThemeId: string | null = null;
let appliedThemeNames: string[] = themeTokenNames(activeTheme);
let themeLoadSeq = 0;

function clearStaleThemeTokens(next: Record<string, string>): void {
  const root = document.documentElement;
  for (const name of appliedThemeNames) {
    if (!(name in next)) root.style.removeProperty(name);
  }
}

function setThemeTokens(tokens: Record<string, string>): void {
  const root = document.documentElement;
  for (const [name, value] of Object.entries(tokens)) root.style.setProperty(name, value);
  appliedThemeNames = Object.keys(tokens);
}

export function currentThemeId(): string {
  if (preferences.value.status === 'loaded') {
    const raw = preferences.value.data[THEME_KEY];
    if (raw) return raw;
  }
  return DEFAULT_THEME_ID;
}

function applyTheme(theme: ResolvedTheme): void {
  activeTheme = theme;
  localStorage.setItem(THEME_STORAGE_KEY, JSON.stringify(theme));
  applyThemeMode(lastAppliedThemeMode);
  if (preferences.value.status === 'loaded') applyFontFamily(currentFontFamily());
}

let refusedThemeWarned: string | null = null;


export async function refreshActiveTheme(
  id: string = currentThemeId(),
  picked = false,
  known?: ResolvedTheme,
): Promise<void> {
  const seq = ++themeLoadSeq;
  requestedThemeId = id;
  if (id === DEFAULT_THEME_ID) {
    applyTheme(EMPTY_THEME);
    return;
  }
  if (known) applyTheme(sanitizeResolvedTheme(known));
  try {
    const theme = await getTheme(id);
    if (seq !== themeLoadSeq) return;
    if (refusedThemeWarned === id) refusedThemeWarned = null;
    const fetched = sanitizeResolvedTheme(theme.resolved);
    if (!known || JSON.stringify(fetched) !== JSON.stringify(activeTheme)) applyTheme(fetched);
  } catch (e) {
    if (seq !== themeLoadSeq) return;
    if (e instanceof ApiError && (e.httpCode === 404 || e.httpCode === 400)) {
      applyTheme(EMPTY_THEME);
      return;
    }
    if (e instanceof ApiError && e.httpCode === 422) {
      applyTheme(EMPTY_THEME);
      if (picked || refusedThemeWarned !== id) {
        refusedThemeWarned = id;
        showToast(`The theme "${id}" no longer passes validation, so the default theme shows: ${errorDetail(e)}`, 'warning');
      }
      return;
    }
    requestedThemeId = null;
    if (picked && !known) {
      showToast(`Could not load the theme "${id}": ${errorDetail(e)}`, 'error');
      return;
    }
    console.warn(`[themes] could not load theme "${id}"`, e);
  }
}

function refreshActiveThemeIfChanged(): void {
  if (currentThemeId() === requestedThemeId) return;
  void refreshActiveTheme();
}

export function isActiveThemePath(path: string | undefined): boolean {
  return path === `themes/${currentThemeId()}.json`;
}

export function setTheme(id: string, known?: ResolvedTheme): Promise<void> {
  return savePreference(THEME_KEY, id, () => void refreshActiveTheme(id, true, known), true);
}


const systemThemeModeFollower = followSystemThemeMode({
  visible: () => document.visibilityState === 'visible',
  moved: () => currentThemeMode() === 'system'
    && document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE) !== resolveThemeMode('system', osPrefersLight()),
  apply: () => {
    const light = osPrefersLight();
    paintThemeMode('system', resolveThemeMode('system', light), light);
  },
});

const SYSTEM_THEME_MODE_RESUME_EVENTS: ReadonlyArray<readonly [EventTarget, string]> = [
  [document, 'visibilitychange'],
  [window, 'focus'],
  [window, 'pageshow'],
];

function syncSystemThemeModeListener(mode: ThemeMode): void {
  const { schedule } = systemThemeModeFollower;
  if (mode === 'system' && systemThemeModeQuery !== null) return;
  systemThemeModeQuery?.removeEventListener('change', schedule);
  systemThemeModeQuery = null;
  for (const [target, type] of SYSTEM_THEME_MODE_RESUME_EVENTS) {
    target.removeEventListener(type, schedule);
  }
  if (mode !== 'system') {
    systemThemeModeFollower.cancel();
    return;
  }

  systemThemeModeQuery = window.matchMedia('(prefers-color-scheme: light)');
  systemThemeModeQuery.addEventListener('change', schedule);
  for (const [target, type] of SYSTEM_THEME_MODE_RESUME_EVENTS) {
    target.addEventListener(type, schedule);
  }
  schedule();
}

syncSystemThemeModeListener(lastAppliedThemeMode);

export function currentThemeMode(): ThemeMode {
  return currentPreference(THEME_MODE_KEY, THEME_MODE_STORAGE_KEY);
}

export function setThemeMode(mode: ThemeMode): Promise<void> {
  return savePreference(THEME_MODE_KEY, mode, () => applyThemeMode(mode), true);
}


function knownWorkspaceFonts(): WorkspaceFont[] {
  const list = workspaceFontList.value;
  const cached = parseWorkspaceFont(localStorage.getItem(WORKSPACE_FONT_STORAGE_KEY));
  const installed = list.status === 'loaded' ? list.data.fonts : cached ? [cached] : [];
  return [...installed, ...activeTheme.workspace_fonts];
}

export function applyFontFamily(preference: FontPreference): void {
  const known = knownWorkspaceFonts();
  const font = resolveFont(preference, activeTheme.fonts, known);
  localStorage.setItem(FONT_FAMILY_STORAGE_KEY, preference);
  if (font.workspaceFont) {
    localStorage.setItem(WORKSPACE_FONT_STORAGE_KEY, JSON.stringify(font.workspaceFont));
  } else if (workspaceFontList.value.status === 'loaded') {
    localStorage.removeItem(WORKSPACE_FONT_STORAGE_KEY);
  }
  registerFontsInUse(font, known, activeTheme.fonts.mono, dataMountUrl);
  document.documentElement.style.setProperty('--font-ui', font.stack);
  document.documentElement.style.setProperty('--font-features-text', font.features.text);
  document.documentElement.style.setProperty('--font-features-code', font.features.code);
  document.documentElement.setAttribute(FONT_BOLD_ATTRIBUTE, fontBoldMark(font));
  reapplyStyleOverrides();
  appearanceVersion.value++;
}

export function currentFontFamily(): FontPreference {
  if (preferences.value.status === 'loaded') {
    const raw = preferences.value.data['font-family'];
    if (isWorkspaceFontId(raw)) return raw;
    const known = FONT_PREFERENCES.find((font) => font === raw);
    if (known) return known;
  }
  return DEFAULT_FONT_PREFERENCE;
}

effect(() => {
  if (workspaceFontList.value.status !== 'loaded') return;
  untracked(() => {
    if (preferences.value.status === 'loaded') applyFontFamily(currentFontFamily());
  });
});

export function setFontFamily(font: FontPreference): Promise<void> {
  return savePreference('font-family', font, () => applyFontFamily(font), true);
}


let appliedOverrideNames: string[] = [];

export function applyStyleOverrides(map: Record<string, string>): void {
  const root = document.documentElement;
  const valid: Record<string, string> = {};
  for (const [name, value] of Object.entries(map)) {
    if (isAllowedOverride(name, value)) valid[name] = value;
  }
  appliedOverrideNames = replaceInlineTokens(
    root.style, appliedOverrideNames, valid, activeTheme[paintedThemeMode.value],
  );
  localStorage.setItem(STYLE_OVERRIDES_STORAGE_KEY, serializeStyleOverrides(map));
  paintThemeColor();
  appearanceVersion.value++;
  scheduleScaleMeasurements();
}

export function reapplyStyleOverrides(): void {
  if (appliedOverrideNames.length === 0) return;
  const root = document.documentElement;
  const map = currentStyleOverrides();
  for (const [name, value] of Object.entries(map)) {
    root.style.setProperty(name, value);
  }
}

export function currentStyleOverrides(): Record<string, string> {
  if (preferences.value.status !== 'loaded') {
    return parseStyleOverrides(localStorage.getItem(STYLE_OVERRIDES_STORAGE_KEY));
  }
  return parseStyleOverrides(preferences.value.data[STYLE_OVERRIDES_KEY]);
}

export function clearStyleOverrides(): Promise<void> {
  return savePreference(STYLE_OVERRIDES_KEY, '{}', () => applyStyleOverrides({}), true);
}

let styleResetHonoured = false;

function applyStyleOverridesFromPreferences(): void {
  try {
    const search = typeof window !== 'undefined' ? (window.location?.search ?? '') : '';
    if (!styleResetHonoured && styleResetRequested(search)) {
      styleResetHonoured = true;
      dropStyleResetParam();
      void clearStyleOverrides().catch((e) => console.warn('[style-remote] reset write failed', e));
      if (currentThemeId() !== DEFAULT_THEME_ID) {
        void setTheme(DEFAULT_THEME_ID).catch((e) => console.warn('[style-remote] theme reset write failed', e));
      }
      return;
    }
    applyStyleOverrides(currentStyleOverrides());
  } catch (e) {
    console.warn('[style-remote] applying overrides failed', e);
  }
}

function dropStyleResetParam(): void {
  if (typeof window === 'undefined' || !window.history?.replaceState) return;
  const url = new URL(window.location.href);
  url.searchParams.delete(STYLE_RESET_PARAM);
  window.history.replaceState(null, '', `${url.pathname}${url.search}${url.hash}`);
}


export function currentImageModel(): ImageModel {
  return currentPreference('image_model');
}

export function setImageModel(model: ImageModel): Promise<void> {
  return savePreference('image_model', model);
}


export function currentNotificationsFilter(): NotificationsFilter {
  return currentPreference('notifications_filter', NOTIFICATIONS_FILTER_STORAGE_KEY);
}


const NOTIFICATION_TOASTS_KEY = 'lucidos-notification-toasts';

export function currentNotificationToasts(): boolean {
  return currentPreference('notification_toasts', NOTIFICATION_TOASTS_KEY) === 'true';
}

export function setNotificationToasts(enabled: boolean): Promise<void> {
  const value = enabled ? 'true' : 'false';
  return savePreference('notification_toasts', value, () => {
    localStorage.setItem(NOTIFICATION_TOASTS_KEY, value);
  });
}


const COMPOSER_ENTER_KEY_STORAGE_KEY = 'lucidos-composer-enter-key';

export type ComposerEnterKey = PreferenceValues<'composer_enter_key'>;

export function currentComposerEnterKey(): ComposerEnterKey {
  return currentPreference('composer_enter_key', COMPOSER_ENTER_KEY_STORAGE_KEY);
}

export function setComposerEnterKey(mode: ComposerEnterKey): Promise<void> {
  return savePreference('composer_enter_key', mode, () => {
    localStorage.setItem(COMPOSER_ENTER_KEY_STORAGE_KEY, mode);
  });
}


export function currentContextCeiling(): string {
  return currentPreference('coding_agent_context_ceiling');
}

export function setContextCeiling(value: string): Promise<void> {
  return savePreference('coding_agent_context_ceiling', value);
}

export function setLandingSetting(value: LandingSetting): Promise<void> {
  return savePreference('landing_setting', value);
}


export function currentMainCheckSetting(): MainCheckSetting | null {
  if (preferences.value.status !== 'loaded') return null;
  return acceptedValue('main_check_setting', preferences.value.data.main_check_setting) as MainCheckSetting | null;
}

export function setMainCheckSetting(value: MainCheckSetting): Promise<void> {
  return savePreference('main_check_setting', value);
}


export function currentAutocorrect(): boolean {
  return currentPreference('autocorrect', AUTOCORRECT_STORAGE_KEY) === 'true';
}

export function setAutocorrect(enabled: boolean): Promise<void> {
  const value = enabled ? 'true' : 'false';
  return savePreference('autocorrect', value, () => {
    localStorage.setItem(AUTOCORRECT_STORAGE_KEY, value);
    setProseAutocorrect(enabled);
  }, true);
}


export type AgentIconColors = PreferenceValues<'agent-icon-colors'>;

const AGENT_ICON_COLORS_STORAGE_KEY = 'lucidos-agent-icon-colors';

export function currentAgentIconColors(): AgentIconColors {
  return currentPreference('agent-icon-colors', AGENT_ICON_COLORS_STORAGE_KEY);
}

export function setAgentIconColors(look: AgentIconColors): Promise<void> {
  return savePreference('agent-icon-colors', look, () => {
    localStorage.setItem(AGENT_ICON_COLORS_STORAGE_KEY, look);
  }, true);
}

effect(() => {
  if (preferences.value.status === 'loaded') {
    cacheServedValue('agent-icon-colors', AGENT_ICON_COLORS_STORAGE_KEY);
  }
  const look = currentAgentIconColors();
  if (typeof document !== 'undefined') {
    document.documentElement.setAttribute('data-agent-icon-colors', look);
  }
});


export function currentMotion(): MotionPref {
  return currentPreference('motion', MOTION_STORAGE_KEY);
}

export function setMotion(pref: MotionPref): Promise<void> {
  return savePreference('motion', pref, () => {
    localStorage.setItem(MOTION_STORAGE_KEY, pref);
    motionPreference.value = pref;
  }, true);
}


export function currentThemeEffects(): ThemeEffectsPref {
  return currentPreference('theme-effects', THEME_EFFECTS_STORAGE_KEY);
}

export function setThemeEffects(pref: ThemeEffectsPref): Promise<void> {
  return savePreference('theme-effects', pref, () => {
    localStorage.setItem(THEME_EFFECTS_STORAGE_KEY, pref);
    themeEffectsPreference.value = pref;
  }, true);
}


export function currentChatModel(): string {
  return currentPreference('chat_model');
}

export function storedChatModel(): string | null {
  if (preferences.value.status !== 'loaded') return null;
  return acceptedValue('chat_model', preferences.value.data.chat_model);
}

function storedChatEfforts(): string | null {
  if (preferences.value.status !== 'loaded') return null;
  return acceptedValue('chat_reasoning_efforts', preferences.value.data.chat_reasoning_efforts);
}

export function accountChatEffort(modelId: string): string | null {
  const stored = parseChatEfforts(storedChatEfforts()).get(modelId);
  return stored === undefined ? modelDefaultEffort(modelId) : clampEffortFor(stored, modelId);
}

async function saveModelEffort(model: string, effort: string | null): Promise<void> {
  if (preferences.value.status !== 'loaded') {
    if (effort !== null) {
      showToast('Preferences have not loaded, so the reasoning effort was not saved', 'error');
    }
    return;
  }
  const stored = storedChatEfforts();
  const next = withChatEffort(stored, model, effort);
  if (next !== (stored ?? '')) await savePreference('chat_reasoning_efforts', next);
}

export async function setChatModelSelection(
  patch: { model: string; reasoningEffort: string | null },
): Promise<void> {
  await savePreference('chat_model', patch.model, () => { currentModel.value = patch.model; });
  await saveModelEffort(patch.model, patch.reasoningEffort);
}

export function currentTriggerModel(): string {
  return currentPreference('trigger_model');
}

export async function setTriggerModelSelection(
  patch: { model: string; reasoningEffort: string | null },
): Promise<void> {
  await savePreference('trigger_model', patch.model);
  await saveModelEffort(patch.model, patch.reasoningEffort);
}


export function currentResponseStyle(): string {
  return currentPreference('response_style');
}

export function setResponseStyle(id: string): Promise<void> {
  return savePreference('response_style', id);
}

export const TECHNICAL_LITERACY_NOT_SET = PREF_TECHNICAL_LITERACY.fallback;

export type TechnicalLiteracy = Exclude<PreferenceValues<'technical_literacy'>, typeof TECHNICAL_LITERACY_NOT_SET>;
export const TECHNICAL_LITERACY_LEVELS: readonly TechnicalLiteracy[] = PREF_TECHNICAL_LITERACY.values
  .filter((level): level is TechnicalLiteracy => level !== TECHNICAL_LITERACY_NOT_SET);

const MERGED_EVERYDAY = 'everyday';

export function currentTechnicalLiteracy(): TechnicalLiteracy | null {
  if (preferences.value.status !== 'loaded') return null;
  const v = preferences.value.data['technical_literacy']?.trim();
  if (v === MERGED_EVERYDAY) return 'non-technical';
  return TECHNICAL_LITERACY_LEVELS.find((level) => level === v) ?? null;
}

export function setTechnicalLiteracy(level: TechnicalLiteracy | null): Promise<void> {
  return savePreference('technical_literacy', level ?? TECHNICAL_LITERACY_NOT_SET);
}


export const MAX_TOOL_CALLS_DEFAULT = Number(PREF_MAX_TOOL_CALLS.fallback);

export const MAX_TOOL_CALLS_MIN = PREF_MAX_TOOL_CALLS.min;

export const MAX_TOOL_CALLS_MAX = PREF_MAX_TOOL_CALLS.max;

const SECONDS_PER_TOOL_CALL = 15;

export function estimateTurnDuration(maxToolCalls: number): string {
  const minutes = (maxToolCalls * SECONDS_PER_TOOL_CALL) / 60;
  if (minutes < 60) return `${Math.max(1, Math.round(minutes))} min`;
  const hours = minutes / 60;
  if (hours < 48) return `${hours < 10 ? hours.toFixed(1).replace(/\.0$/, '') : Math.round(hours)} hours`;
  return `${Math.round(hours / 24)} days`;
}

export function currentMaxToolCalls(): number {
  if (preferences.value.status !== 'loaded') return MAX_TOOL_CALLS_DEFAULT;
  const raw = preferences.value.data['max_tool_calls'];
  if (raw == null) return MAX_TOOL_CALLS_DEFAULT;
  const parsed = Number(raw.trim());
  if (raw.trim() === '' || !(parsed >= MAX_TOOL_CALLS_MIN && parsed <= MAX_TOOL_CALLS_MAX)) {
    return MAX_TOOL_CALLS_DEFAULT;
  }
  return Math.trunc(parsed);
}

export function setMaxToolCalls(maxToolCalls: number): Promise<void> {
  return savePreference('max_tool_calls', String(maxToolCalls));
}


export function currentLanguage(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['language'] || '';
}

export function setLanguage(language: string): Promise<void> {
  return savePreference('language', language.trim());
}

export function currentTimezone(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['timezone'] || '';
}

export function setTimezone(timezone: string): Promise<void> {
  return savePreference('timezone', timezone);
}


let preferencesLoadSeq = 0;
let landedPreferencesSeq = 0;
let overtakenPreferencesLoads: Array<() => void> = [];

function newestPreferencesReadLands(): Promise<void> | undefined {
  if (landedPreferencesSeq === preferencesLoadSeq) return undefined;
  return new Promise((resolve) => { overtakenPreferencesLoads.push(resolve); });
}

function preferencesReadLanded(seq: number): void {
  landedPreferencesSeq = seq;
  const waiting = overtakenPreferencesLoads;
  overtakenPreferencesLoads = [];
  for (const resolve of waiting) resolve();
}

export function loadPreferences(): Promise<void> {
  return startSliceLoad('preferences') ?? loadPreferencesAs(preferencesSlice.begin());
}

export const preferencesSettled = computed(() => {
  const { status } = preferences.value;
  return status === 'loaded' || status === 'failed';
});

interface PreferencesRead {
  seq: number;
  unansweredAtStart: Array<[string, PendingPreferenceWrite]>;
}

async function loadPreferencesAs(read: PreferencesRead): Promise<void> {
  try {
    const res = await retryTransient(() => getPreferences(getDeviceId()));
    if (read.seq !== preferencesLoadSeq) return newestPreferencesReadLands();
    preferencesSlice.apply(res, read);
  } catch (e) {
    if (read.seq !== preferencesLoadSeq) return newestPreferencesReadLands();
    preferencesSlice.fail(e, read);
  }
}

export const preferencesSlice: SliceOf<'preferences', PreferencesRead> = {
  begin: () => {
    if (preferences.value.status === 'not-loaded') {
      preferences.value = { status: 'loading' };
    }
    return { seq: ++preferencesLoadSeq, unansweredAtStart: [...unansweredWrites] };
  },
  apply: (res, { seq, unansweredAtStart }) => {
    if (seq !== preferencesLoadSeq) return;
    const refreshing = preferences.value.status === 'loaded';
    preferences.value = {
      status: 'loaded',
      data: {
        ...res.preferences,
        ...writeValues(unansweredAtStart.filter(([key, write]) => refusedWriteSeq.get(key) !== write.seq)),
        ...writeValues(unansweredWrites),
      },
    };
    applyUiScale(previewedUiScale ?? currentUiScale());
    const t = currentThemeMode();
    if (t !== lastAppliedThemeMode) applyThemeMode(t);
    applyFontFamily(currentFontFamily());
    if (workspaceFontList.value.status === 'not-loaded') void loadWorkspaceFonts();
    currentModel.value = currentChatModel();
    notificationsFilter.value = currentNotificationsFilter();
    cacheServedValue('notification_toasts', NOTIFICATION_TOASTS_KEY);
    cacheServedValue('composer_enter_key', COMPOSER_ENTER_KEY_STORAGE_KEY);
    cacheServedValue('autocorrect', AUTOCORRECT_STORAGE_KEY);
    setProseAutocorrect(currentAutocorrect());
    cacheServedValue('motion', MOTION_STORAGE_KEY);
    motionPreference.value = currentMotion();
    cacheServedValue('theme-effects', THEME_EFFECTS_STORAGE_KEY);
    themeEffectsPreference.value = currentThemeEffects();
    if (refreshing) applyCodingAgentDefault(currentCodingAgentDefault());
    else selectedCodingAgent.value = currentCodingAgentDefault();
    refreshActiveThemeIfChanged();
    applyStyleOverridesFromPreferences();
    preferencesReadLanded(seq);
  },
  fail: (e, { seq }) => {
    if (seq !== preferencesLoadSeq) return;
    preferences.value = failedIfFresh(preferences.value, e);
    preferencesReadLanded(seq);
  },
};


export function currentVertexRegion(): string {
  return currentPreference('vertex_region');
}

export function setVertexRegion(region: string): Promise<void> {
  return savePreference('vertex_region', region);
}


export const DEFAULT_LOCAL_BASE_URL = PREF_LOCAL_BASE_URL.fallback;

export function currentLocalBaseUrl(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['local_base_url'] || '';
}

export function setLocalBaseUrl(url: string): Promise<void> {
  return savePreference('local_base_url', url.trim());
}

export function currentClaudeGatewayUrl(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['claude_gateway_url'] || '';
}

export function chatGptPlanWelcomeShown(): boolean {
  return currentPreference('chatgpt_plan_welcome_shown') === 'true';
}

export function markChatGptPlanWelcomeShown(): Promise<void> {
  return savePreference('chatgpt_plan_welcome_shown', 'true');
}


export function currentOpenCodeFreeEnabled(): boolean {
  return currentPreference('opencode_free_enabled') === 'true';
}

export function setOpenCodeFreeEnabled(enabled: boolean): Promise<void> {
  return savePreference('opencode_free_enabled', enabled ? 'true' : 'false');
}


export type SwitchableProvider =
  | 'vertex' | 'anthropic' | 'openai' | 'openrouter' | 'xai' | 'local' | 'claude-gateway'
  | 'chatgpt-plan';

function providerEnabledKey(id: SwitchableProvider): string {
  return `provider_enabled_${id.replace('-', '_')}`;
}

export function providerSwitchedOff(id: SwitchableProvider): boolean {
  if (preferences.value.status !== 'loaded') return false;
  return preferences.value.data[providerEnabledKey(id)] === 'false';
}

export function setProviderEnabled(
  id: SwitchableProvider,
  enabled: boolean,
): Promise<void> {
  return savePreference(providerEnabledKey(id), enabled ? 'true' : 'false');
}

export const PROVIDER_ENABLED_TYPESAFE_KEY = PREF_PROVIDER_ENABLED_TYPESAFE.key;
export const PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI_KEY = PREF_PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI.key;
export const PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM_KEY = PREF_PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM.key;

export const SYSTEM_ONE_CUSTOM_URL_KEY = PREF_SYSTEM_ONE_CUSTOM_URL.key;
export const SYSTEM_ONE_CUSTOM_MODEL_KEY = PREF_SYSTEM_ONE_CUSTOM_MODEL.key;

export const PROVIDER_ENABLED_OPENROUTER_KEY = PREF_PROVIDER_ENABLED_OPENROUTER.key;
export const PROVIDER_ENABLED_OPENAI_KEY = PREF_PROVIDER_ENABLED_OPENAI.key;

export const SYSTEM_ONE_OPENROUTER_MODEL_KEY = PREF_SYSTEM_ONE_OPENROUTER_MODEL.key;

export function switchValueReadsAsOff(raw: string | undefined): boolean {
  return raw !== undefined && flagReading(raw) === 'false';
}

export function setSystemOneEnabled(switchKey: string, enabled: boolean): Promise<void> {
  return savePreference(switchKey, enabled ? 'true' : 'false');
}


export function currentCaptureContext(): boolean {
  return currentPreference('capture_context') === 'true';
}

export function setCaptureContext(enabled: boolean): Promise<void> {
  return savePreference('capture_context', enabled ? 'true' : 'false');
}

export function currentTerminalEnabled(): boolean {
  return currentPreference('terminal_enabled') === 'true';
}

export function setTerminalEnabled(enabled: boolean): Promise<void> {
  return savePreference('terminal_enabled', enabled ? 'true' : 'false');
}


export function currentCacheCards(): boolean {
  return currentCacheKeepAliveOn() && currentPreference('cache_cards') === 'true';
}

export function setCacheCards(enabled: boolean): Promise<void> {
  return savePreference('cache_cards', enabled ? 'true' : 'false');
}


export function currentInAppBrowser(): boolean {
  if (preferences.value.status !== 'loaded') return false;
  return preferences.value.data['experimental_in_app_browser'] === 'true';
}

export function setInAppBrowser(enabled: boolean): Promise<void> {
  return savePreference('experimental_in_app_browser', enabled ? 'true' : 'false');
}

export function inAppBrowserAvailable(): boolean {
  return isTauri() && currentInAppBrowser();
}


export type ExternalLinkTarget = PreferenceValues<'external_link_target'>;

export function currentExternalLinkTarget(): ExternalLinkTarget {
  return currentPreference('external_link_target');
}

export function setExternalLinkTarget(target: ExternalLinkTarget): Promise<void> {
  return savePreference('external_link_target', target);
}

export function externalLinkTargetConfigurable(): boolean {
  return isIOSPwa();
}


export function currentMobileDynamicBars(): boolean {
  return isIOSPwa() || currentPreference('mobile_dynamic_bars') === 'true';
}

export function setMobileDynamicBars(enabled: boolean): Promise<void> {
  return savePreference('mobile_dynamic_bars', enabled ? 'true' : 'false');
}


export type BackgroundModelKey =
  | 'model_title'
  | 'model_change_summary'
  | 'model_image_description'
  | 'model_memory'
  | 'model_conversation_summary'
  | 'model_query_classification'
  | 'model_summary_compaction'
  | 'model_command_judge'
  | 'model_read_decision'
  | 'model_wake_decision'
  | 'model_expected_wake_decision'
  | 'model_permission_answer';

export type BackgroundReasoningKey =
  | 'reasoning_title'
  | 'reasoning_change_summary'
  | 'reasoning_image_description'
  | 'reasoning_memory'
  | 'reasoning_conversation_summary'
  | 'reasoning_query_classification'
  | 'reasoning_summary_compaction'
  | 'reasoning_command_judge'
  | 'reasoning_read_decision'
  | 'reasoning_wake_decision'
  | 'reasoning_expected_wake_decision'
  | 'reasoning_permission_answer';

export function storedBackgroundSelection(
  modelKey: BackgroundModelKey,
  reasoningKey: BackgroundReasoningKey,
): { model: string | null; effort: string | null } {
  const data = preferences.value.status === 'loaded' ? preferences.value.data : {};
  return {
    model: acceptedValue(modelKey, data[modelKey]),
    effort: acceptedValue(reasoningKey, data[reasoningKey]),
  };
}

export function storedTitleStyle(): string {
  const data = preferences.value.status === 'loaded' ? preferences.value.data : {};
  return data[PREF_TITLE_STYLE.key] ?? '';
}

export async function saveModelSelection(
  modelKey: BackgroundModelKey,
  reasoningKey: BackgroundReasoningKey,
  patch: { model: string; reasoningEffort: string | null },
): Promise<void> {
  await savePreference(modelKey, patch.model);
  if (patch.reasoningEffort !== null) await savePreference(reasoningKey, patch.reasoningEffort);
}


export type MemoryModule = PreferenceValues<'memory_module'>;

export function storedMemoryModule(): MemoryModule | null {
  if (preferences.value.status !== 'loaded') return null;
  return acceptedValue('memory_module', preferences.value.data.memory_module) as MemoryModule | null;
}

export function currentMemoryModule(): MemoryModule {
  return storedMemoryModule() ?? MEMORY_MODULE_WHILE_UNSET;
}

export function setMemoryModule(module: MemoryModule): Promise<void> {
  return savePreference('memory_module', module);
}


export function currentCodingAgentDefault(): CodingAgent {
  return currentPreference('coding_agent_default');
}

export const CC_PERMISSION_MODES = PREF_CODING_AGENT_CLAUDE_PERMISSION_MODE.values;
export type CcPermissionMode = PreferenceValues<'coding_agent_claude_permission_mode'>;

export function currentCodingAgentPermissionMode(): CcPermissionMode {
  return currentPreference('coding_agent_claude_permission_mode');
}

export function setCodingAgentPermissionMode(mode: CcPermissionMode): Promise<void> {
  return savePreference('coding_agent_claude_permission_mode', mode);
}

export function currentCacheKeepAliveOn(): boolean {
  return currentPreference('cache_keep_alive_mode') === 'on';
}

export function setCacheKeepAliveOn(on: boolean): Promise<void> {
  return savePreference('cache_keep_alive_mode', on ? 'on' : 'off');
}


export function welcomeSuggestionsDismissed(): boolean {
  return welcomeDismissed.value;
}

export function dismissWelcomeSuggestions(): Promise<void> {
  if (preferences.value.status === 'loaded'
    && preferences.value.data['welcome_suggestions_dismissed'] === 'true') {
    return Promise.resolve();
  }
  return savePreference('welcome_suggestions_dismissed', 'true');
}

export const WELCOME_RETIRES_AFTER_THREADS = 3;

export function retireWelcomeAfterUse(threads: Iterable<ThreadState>): Promise<void> {
  if (!llmConfigured.value || welcomeSuggestionsDismissed()) return Promise.resolve();
  let started = 0;
  for (const { meta } of threads) {
    if (meta.initiator === 'user' && meta.state === 'active' && !meta.parentThreadId && !meta.home) started++;
  }
  return started >= WELCOME_RETIRES_AFTER_THREADS ? dismissWelcomeSuggestions() : Promise.resolve();
}


export function backupIsConfigured(prefs: Record<string, string>): boolean {
  return !!prefs['backup_provider'];
}

export const BACKUP_REMINDER_SNOOZE_MS = 30 * 24 * 60 * 60 * 1000;

export const BACKUP_REMINDER_FOREVER = 'forever';

export function backupReminderHiddenByDismissal(value: string | undefined, now: number): boolean {
  if (!value) return false;
  if (value === BACKUP_REMINDER_FOREVER) return true;
  const at = Date.parse(value);
  if (Number.isNaN(at)) return false;
  return now - at < BACKUP_REMINDER_SNOOZE_MS;
}

export function backupReminderNextDismissal(value: string | undefined, now: number): string {
  if (value === BACKUP_REMINDER_FOREVER) return BACKUP_REMINDER_FOREVER;
  const dismissedOnce = !!value && !Number.isNaN(Date.parse(value));
  return dismissedOnce ? BACKUP_REMINDER_FOREVER : new Date(now).toISOString();
}

export function backupReminderVisibleIn(prefs: Record<string, string>, now: number): boolean {
  if (backupIsConfigured(prefs)) return false;
  return !backupReminderHiddenByDismissal(prefs['backup_reminder_dismissed'], now);
}

export const BACKUP_OVERDUE_SNOOZE_MS = 7 * 24 * 60 * 60 * 1000;

export function backupOverdueHiddenByDismissal(value: string | undefined, now: number): boolean {
  if (!value) return false;
  const at = Date.parse(value);
  return !Number.isNaN(at) && now - at < BACKUP_OVERDUE_SNOOZE_MS;
}

export type BackupBanner =
  | { state: 'setup' }
  | { state: 'overdue'; provider: string; lastBackupAt: string | null };

function backupOverdueAt(verdict: LastSuccessfulBackupResponse, now: number): boolean {
  return verdict.overdue || (verdict.overdue_at !== null && now > Date.parse(verdict.overdue_at));
}

export function backupBannerIn(
  prefs: Record<string, string>,
  lastSuccessful: Loadable<LastSuccessfulBackupResponse>,
  now: number,
): BackupBanner | null {
  if (backupReminderVisibleIn(prefs, now)) return { state: 'setup' };
  const provider = prefs['backup_provider'];
  if (!provider) return null;
  if (lastSuccessful.status !== 'loaded' || !backupOverdueAt(lastSuccessful.data, now)) return null;
  if (backupOverdueHiddenByDismissal(prefs['backup_overdue_reminder_dismissed'], now)) return null;
  return { state: 'overdue', provider, lastBackupAt: lastSuccessful.data.at };
}

export function backupBanner(now: number = Date.now()): BackupBanner | null {
  if (preferences.value.status !== 'loaded') return null;
  return backupBannerIn(preferences.value.data, lastSuccessfulBackup.value, now);
}

export function dismissBackupReminder(now: number = Date.now()): Promise<void> {
  if (preferences.value.status !== 'loaded') return Promise.resolve();
  const next = backupReminderNextDismissal(
    preferences.value.data['backup_reminder_dismissed'],
    now,
  );
  return savePreference('backup_reminder_dismissed', next);
}

export function dismissBackupOverdueReminder(now: number = Date.now()): Promise<void> {
  if (preferences.value.status !== 'loaded') return Promise.resolve();
  return savePreference('backup_overdue_reminder_dismissed', new Date(now).toISOString());
}


export function currentCommandGuard(): boolean {
  return currentPreference('command_guard') === 'true';
}

export function setCommandGuard(enabled: boolean): Promise<void> {
  return savePreference('command_guard', enabled ? 'true' : 'false');
}

export function currentCommandGuardJudge(): boolean {
  return currentPreference('command_guard_judge') === 'true';
}

export function setCommandGuardJudge(enabled: boolean): Promise<void> {
  return savePreference('command_guard_judge', enabled ? 'true' : 'false');
}

export function currentCodingAgentAutoAnswer(): boolean {
  return currentPreference('coding_agent_auto_answer') === 'true';
}

export function setCodingAgentAutoAnswer(enabled: boolean): Promise<void> {
  return savePreference('coding_agent_auto_answer', enabled ? 'true' : 'false');
}

export function currentAgentsUnsandboxed(): boolean {
  return currentPreference('agents_unsandboxed') === 'true';
}

export function setAgentsUnsandboxed(enabled: boolean): Promise<void> {
  return savePreference('agents_unsandboxed', enabled ? 'true' : 'false');
}


export function voiceEnabled(): boolean {
  return currentPreference('voice_enabled') === 'true';
}

export function setVoiceEnabled(enabled: boolean): Promise<void> {
  return savePreference('voice_enabled', enabled ? 'true' : 'false');
}

export const DEFAULT_VOICE_TALKER_MODEL = PREF_MODEL_VOICE_TALKER.fallback;

export function storedVoiceTalkerModel(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['model_voice_talker'] ?? '';
}

export function currentVoiceTalkerModel(): string {
  return currentPreference('model_voice_talker');
}

export function setVoiceTalkerModel(model: string): Promise<void> {
  return savePreference('model_voice_talker', model.trim());
}

export const DEFAULT_VOICE_TRANSCRIBER_MODEL = PREF_MODEL_VOICE_TRANSCRIBER.fallback;

export function storedVoiceTranscriberModel(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['model_voice_transcriber'] ?? '';
}

export function currentVoiceTranscriberModel(): string {
  return currentPreference('model_voice_transcriber');
}

export function setVoiceTranscriberModel(model: string): Promise<void> {
  return savePreference('model_voice_transcriber', model.trim());
}

export const DEFAULT_VOICE_TALKER_VOICE = PREF_VOICE_TALKER_VOICE.fallback;

export function storedVoiceTalkerVoice(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['voice_talker_voice'] ?? '';
}

export function currentVoiceTalkerVoice(): string {
  return currentPreference('voice_talker_voice');
}

export function setVoiceTalkerVoice(voice: string): Promise<void> {
  return savePreference('voice_talker_voice', voice.trim());
}

export { VOICE_RESIDENT_SECTIONS };

export const DEFAULT_VOICE_RESIDENT_SECTIONS: readonly string[] =
  PREF_VOICE_RESIDENT_SECTIONS.fallback.split(',');

function setVoiceResidentSections(sections: string): Promise<void> {
  return savePreference('voice_resident_sections', sections.trim());
}

export function voiceResidentSelection(): string[] | null {
  if (preferences.value.status !== 'loaded') return null;
  const stored = preferences.value.data['voice_resident_sections'];
  if (stored === undefined) return null;
  return stored
    .split(',')
    .map((id) => id.trim())
    .filter(Boolean);
}

export function voiceSectionEnabled(id: string): boolean {
  const selection = voiceResidentSelection();
  if (selection === null) return DEFAULT_VOICE_RESIDENT_SECTIONS.includes(id);
  return selection.includes(id);
}

export function setVoiceSectionEnabled(id: string, on: boolean): Promise<void> {
  const current = new Set(voiceResidentSelection() ?? DEFAULT_VOICE_RESIDENT_SECTIONS);
  if (on) current.add(id);
  else current.delete(id);
  const registry: readonly string[] = VOICE_RESIDENT_SECTIONS.map((s) => s.id);
  const ordered = registry.filter((known) => current.has(known));
  const unknown = [...current].filter((held) => !registry.includes(held));
  return setVoiceResidentSections([...ordered, ...unknown].join(','));
}

export function storedVoiceInputDevice(): string {
  if (preferences.value.status !== 'loaded') return '';
  return preferences.value.data['voice_input_device'] ?? '';
}

export function setVoiceInputDevice(deviceId: string): Promise<void> {
  return savePreference('voice_input_device', deviceId.trim(), undefined, true);
}
