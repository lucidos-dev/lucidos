import { preferences, showToast, removeToast, notificationsFilter, currentModel, reasoningEffort, selectedCodingAgent } from '../store';
import type { CodingAgent } from '../../api/types';
import { toFailed } from '../types';
import { getPreferences, setPreference, isTransientFetchError } from '../../api/client';
import { getDeviceId } from './devices';
import { errorDetail } from '../../utils/errorDetail';
import { createFailureCounter } from '../../utils/failureCounter';
import { REASONING_LEVELS, clampReasoningEffort, DEFAULT_CHAT_MODEL } from '../models';
import { isIOS, isIOSPwa, isTauri } from '../../utils/platform';
import { setTitlebarColor } from '../../utils/tauri';

export type Theme = 'light' | 'dark' | 'system';
export type FontFamily = 'monospace' | 'system' | 'inter' | 'jetbrains-mono' | 'ibm-plex-mono' | 'fira-code';
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
  monospace: "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, 'Fira Code', 'JetBrains Mono', Monaco, Consolas, monospace",
  system: "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
  inter: "'Inter', system-ui, sans-serif",
  'jetbrains-mono': "'JetBrains Mono', monospace",
  'ibm-plex-mono': "'IBM Plex Mono', monospace",
  'fira-code': "'Fira Code', monospace",
};

const GOOGLE_FONT_URLS: Partial<Record<FontFamily, string>> = {
  inter: 'https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap',
  'jetbrains-mono': 'https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;600;700&display=swap',
  'ibm-plex-mono': 'https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600;700&display=swap',
  'fira-code': 'https://fonts.googleapis.com/css2?family=Fira+Code:wght@400;500;600;700&display=swap',
};

// Programming ligatures, enabled ONLY for fonts that ship them and that the
// user explicitly picked for them (Fira Code's signature feature: => != === ).
// Every other font sets `normal` (the CSS initial), so their rendering is
// unchanged. `font-feature-settings` is inherited, so setting it on <html>
// cascades to all text — mirrored in the FOUC scripts (index.html,
// api/sdk_prefs.rs) and the SDK (packages/lucidos-sdk/src/ui.ts).
const FONT_FEATURES: Partial<Record<FontFamily, string>> = {
  'fira-code': '"liga" 1, "calt" 1',
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

// --- Preference writes: apply locally, deliver durably ---
//
// `savePreference` applies the value BEFORE the network call (the side effect
// plus the optimistic signal patch), so the UI is never gated on the round trip.
// That makes delivery the only thing that can still go wrong, and on an
// installed iOS PWA it goes wrong constantly for a reason that says nothing
// about the request: WebKit suspends the page (tens of times a day on a busy
// workspace) and aborts every in-flight fetch. The old code toasted that as
// "Failed to save <key> preference: request cancelled", never retried, and left
// the device showing a value the server never received.

/** A preference write the engine has not accepted yet. Parked once an immediate
 *  re-send has also failed transiently, and flushed on the next page resume.
 *
 *  `seq` is the write's position in the global request order, used to spot one
 *  that a newer value for the same key has superseded. `PUT /preferences?key=<k>`
 *  is a per-key overwrite, so this map is last-write-wins rather than a queue: a
 *  value for a key the user has since changed again is garbage, not backlog.
 *  See `docs/glossary.md` § pending preference write. */
interface PendingPreferenceWrite {
  value: string;
  deviceScoped: boolean;
  seq: number;
}

const pendingPreferenceWrites = new Map<string, PendingPreferenceWrite>();
let writeSeq = 0;

/** The newest `seq` requested per key, in-flight ones included. */
const latestWriteSeq = new Map<string, number>();

/** The tail of each key's delivery chain. Two writes to the same key must never
 *  be in flight at once: the engine applies them in ARRIVAL order, so an older
 *  request landing second silently overwrites the user's latest choice, and the
 *  local `seq` bookkeeping cannot see that happen. Serializing also gives the
 *  supersede check below its meaning, because a queued write re-decides whether
 *  it is still wanted at the moment it would actually go out.
 *
 *  Bounded by the number of distinct preference keys, and each entry is dropped
 *  as soon as its chain goes idle. */
const deliveryChains = new Map<string, Promise<void>>();

function enqueueDelivery(key: string, run: () => Promise<void>): Promise<void> {
  const prior = deliveryChains.get(key) ?? Promise.resolve();
  // `then(run, run)` so one rejected link cannot wedge the key's chain forever.
  const next = prior.then(run, run);
  deliveryChains.set(key, next);
  void next.finally(() => {
    if (deliveryChains.get(key) === next) deliveryChains.delete(key);
  });
  return next;
}

/** The engine REJECTED the write (4xx/5xx, a bad body). A verdict the user is
 *  owed, and one no retry can change. Keyed so repeats collapse into one card. */
const PREFERENCE_REJECTED_TOAST = 'preference-save-rejected';
/** The write never GOT to the engine, repeatedly. Keyed separately from the
 *  verdict above so draining the queue can retract this one without also
 *  clearing a rejection the user still needs to read. */
const PREFERENCE_UNREACHABLE_TOAST = 'preference-save-unreachable';

/** Consecutive writes that got no ANSWER. Silent below the threshold, because
 *  the value is applied locally and a re-send is owed, so one suspended fetch is
 *  noise rather than news. At the threshold it speaks once: a genuinely
 *  unreachable engine must not be swallowed. Reset by any answer, a rejection
 *  included, since a 4xx proves the engine is reachable. */
const writeFailures = createFailureCounter(3, () => {
  const stuck = [...pendingPreferenceWrites.keys()].sort().join(', ');
  showToast(
    `Preference changes are not reaching the engine (${stuck}). They are applied on this device and will be re-sent automatically.`,
    'error',
    { key: PREFERENCE_UNREACHABLE_TOAST },
  );
});

/** The engine answered about this key, so it is no longer owed a re-send. Both
 *  answers land here, accepted and refused alike, because both prove the engine
 *  is reachable: the unreachable banner has to be retracted whichever way the
 *  queue drained, or it keeps insisting nothing is getting through while the
 *  rejection card next to it says otherwise. */
function settleDelivered(key: string): void {
  pendingPreferenceWrites.delete(key);
  writeFailures.recordSuccess();
  if (pendingPreferenceWrites.size === 0) removeToast(PREFERENCE_UNREACHABLE_TOAST);
}

/** Queue one preference write behind any other delivery for the same key. Every
 *  path that talks to the engine about a preference goes through here, so writes
 *  to one key are strictly ordered while different keys still go in parallel. */
function deliverOrPark(key: string, write: PendingPreferenceWrite): Promise<void> {
  return enqueueDelivery(key, () => deliverNow(key, write));
}

/** Send one preference write: retry once on a transient rejection, park it for
 *  the next resume if that fails too, surface a real verdict immediately.
 *
 *  Two attempts rather than one because the two transient causes need different
 *  medicine. A radio handoff or a stale connection heals within milliseconds, so
 *  the immediate re-send usually lands (this is `retryTransientRead`'s bargain,
 *  applied to the most idempotent write in the app). A suspended page does not,
 *  so the second failure hands off to the resume flush instead of burning more
 *  attempts against a webview that is not running.
 *
 *  Runs inside the key's delivery chain, so no other write for this key is in
 *  flight and the map bookkeeping below needs no ordering guards of its own. */
async function deliverNow(key: string, write: PendingPreferenceWrite): Promise<void> {
  for (let attempt = 0; attempt < 2; attempt++) {
    // Re-checked per attempt, not once up front: the user can change the same
    // setting again while we wait our turn OR between our two attempts. The
    // newer value owns the key from that moment, and sending ours would put the
    // stale one on the server after it.
    if ((latestWriteSeq.get(key) ?? write.seq) > write.seq) return;
    try {
      await setPreference(key, write.value, write.deviceScoped ? getDeviceId() : undefined);
    } catch (e) {
      if (isTransientFetchError(e)) continue;
      // The engine ANSWERED and refused. No retry can change that, and the user
      // is owed the reason. It also proves the engine is reachable, so this key
      // stops being owed a re-send and the unreachable count resets.
      settleDelivered(key);
      showToast(`Failed to save ${key} preference: ${errorDetail(e)}`, 'error', {
        key: PREFERENCE_REJECTED_TOAST,
      });
      return;
    }
    // Kept OUT of the try: a throw from the bookkeeping below is not a failed
    // save, and catching it here would report one.
    settleDelivered(key);
    return;
  }
  // Both attempts were cancelled, timed out, or dropped in transit. Park the
  // value and stay quiet: the user sees their change applied, and the resume
  // flush delivers it.
  pendingPreferenceWrites.set(key, write);
  writeFailures.recordFailure();
}

/** Re-send every parked preference write. Called from `useStartup`'s resume
 *  handler, which is the moment a suspended iOS PWA can reach the engine again.
 *  A write that fails transiently here stays parked for the next resume. */
export async function flushPendingPreferenceWrites(): Promise<void> {
  if (pendingPreferenceWrites.size === 0) return;
  // Snapshot first: `deliverNow` mutates the map as each write settles.
  const entries = [...pendingPreferenceWrites.entries()];
  await Promise.all(entries.map(([key, write]) => deliverOrPark(key, write)));
}

/** Test-only: the keys whose latest value the engine has not accepted. */
export function _pendingPreferenceKeysForTesting(): string[] {
  return [...pendingPreferenceWrites.keys()].sort();
}

/** Test-only: drop all parked writes, delivery ordering and the escalation
 *  counter, so one case's undelivered write can't leak into the next. */
export function _resetPendingPreferenceWritesForTesting(): void {
  pendingPreferenceWrites.clear();
  latestWriteSeq.clear();
  deliveryChains.clear();
  writeFailures.recordSuccess();
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
  const seq = ++writeSeq;
  // Claim the key BEFORE queueing, so any write already in flight or waiting its
  // turn can see it has been superseded and stand down.
  latestWriteSeq.set(key, seq);
  await deliverOrPark(key, { value, deviceScoped, seq });
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
  const bg = resolved === 'light' ? '#ffffff' : '#07172e';
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

  // Tauri (packaged macOS): match the reclaimed title-bar band's behind-the-
  // webview fallback (the window background) to the in-app header — the
  // header-gradient top stop per theme (mirrors --header-gradient in
  // styles/global/base.css, like the --bg-primary literal above; the visible
  // band itself is the CSS .titlebar-strip). Best-effort and cosmetic: it runs
  // whenever the theme is applied (incl. startup / system-theme changes) with no
  // user-facing surface, and a failed call self-heals on the next applyTheme, so
  // a toast would be wrong.
  if (isTauri()) {
    const titlebar = resolved === 'light' ? '#1a6fd0' : '#15549e';
    setTitlebarColor(titlebar).catch((e) => console.warn('[titlebar] tint failed', e));
  }

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
  document.documentElement.style.setProperty(
    'font-feature-settings',
    FONT_FEATURES[font] || 'normal',
  );
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

// --- Max tool calls (the per-turn tool-call cap) ---

/** Mirrors `DEFAULT_MAX_TOOL_CALLS` in `core/preferences.rs`. */
export const MAX_TOOL_CALLS_DEFAULT = 500;

/** Mirrors `MIN_MAX_TOOL_CALLS` in `core/preferences.rs`. There is deliberately
 *  no maximum: a high cap costs the user time and tokens, which is their call to
 *  make. The floor rules out only the value that is broken rather than small,
 *  since the loop checks `iterations > cap` after incrementing and a cap of 0
 *  would end the turn before the first LLM call. */
export const MAX_TOOL_CALLS_MIN = 1;

/** The largest cap the UI will write. This is NOT the policy ceiling the design
 *  deliberately omits, it is the point where JavaScript stops being able to
 *  carry the number: past `Number.MAX_SAFE_INTEGER`, `Number('…')` rounds (and
 *  eventually reaches `Infinity`), so `String(Number(input))` would save a
 *  *different* value than the user typed and the engine would then enforce, or
 *  reject, something else again. Nine quadrillion tool calls is not a cap anyone
 *  reaches; the bound exists so the UI cannot display a value the engine will
 *  not honor. */
export const MAX_TOOL_CALLS_REPRESENTABLE = Number.MAX_SAFE_INTEGER;

/** Roughly how long a turn can run at a given cap, in seconds per tool call.
 *  The LLM round-trip dominates a step: ~15s for a large-context reasoning
 *  model, faster on a small model with light tools, slower under heavy `bash`
 *  work. Reads as an upper estimate, because a step that batches several tool
 *  calls pays one round-trip for all of them. Used only to show the user what a
 *  number means before they pick it, so an order of magnitude is the point. */
const SECONDS_PER_TOOL_CALL = 15;

/** A human "about N hours" (or minutes/days) for a cap, for the Settings note.
 *  Deliberately coarse: the point is the order of magnitude, not a promise. */
export function estimateTurnDuration(maxToolCalls: number): string {
  const minutes = (maxToolCalls * SECONDS_PER_TOOL_CALL) / 60;
  if (minutes < 60) return `${Math.max(1, Math.round(minutes))} min`;
  const hours = minutes / 60;
  if (hours < 48) return `${hours < 10 ? hours.toFixed(1).replace(/\.0$/, '') : Math.round(hours)} hours`;
  return `${Math.round(hours / 24)} days`;
}

/** The per-turn tool-call cap. Mirrors `PreferenceStore::max_tool_calls` so the
 *  UI never displays a value the engine would not honor: an absent or
 *  unparseable value shows the default, and a parsed value is raised to the
 *  floor. A large value is shown as stored, since there is no ceiling. */
export function currentMaxToolCalls(): number {
  if (preferences.value.status !== 'loaded') return MAX_TOOL_CALLS_DEFAULT;
  const raw = preferences.value.data['max_tool_calls'];
  if (raw == null) return MAX_TOOL_CALLS_DEFAULT;
  // Integers only, matching the engine's `parse::<usize>()`: it rejects "12.5"
  // and "-5" outright, where `parseInt` would happily read 12 and -5.
  const trimmed = raw.trim();
  if (!/^\d+$/.test(trimmed)) return MAX_TOOL_CALLS_DEFAULT;
  // A value JS cannot carry exactly could only have been written outside this
  // UI (the CLI, the HTTP API, psql), since `setMaxToolCalls` refuses to write
  // one. Show the bound rather than a silently rounded number.
  const parsed = Number(trimmed);
  if (parsed > MAX_TOOL_CALLS_REPRESENTABLE) return MAX_TOOL_CALLS_REPRESENTABLE;
  return Math.max(MAX_TOOL_CALLS_MIN, parsed);
}

export function setMaxToolCalls(maxToolCalls: number): Promise<void> {
  return savePreference('max_tool_calls', String(maxToolCalls));
}

// --- Locale (language + timezone) ---
//
// Both are GLOBAL (workspace-wide, not device-scoped). Writing them goes through
// PUT /preferences → the engine's apply_preference_write chokepoint, which
// refreshes the engine's in-memory user_language/user_timezone and emits
// LanguageSet/TimezoneSet — so the change takes effect with no restart, and the
// frontend live-refreshes (thread-sync reloads preferences on those events).

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

/** Per-step ContextAssembled capture toggle. Defaults to false (off) — the
 *  debugging capture ships dark; only an explicit `'true'` opts in. */
export function currentCaptureContext(): boolean {
  if (preferences.value.status !== 'loaded') return false;
  const raw = preferences.value.data['capture_context'];
  if (raw == null) return false;
  return raw === 'true';
}

export function setCaptureContext(enabled: boolean): Promise<void> {
  return savePreference('capture_context', enabled ? 'true' : 'false');
}

// --- Experimental: in-app browser (Tauri native webview) ---

/** Whether URL previews open in the in-app native webview ("Tauri browser")
 *  instead of the system browser. Experimental and desktop-only — the native
 *  webview exists only under Tauri; web/PWA always opens a new tab. Defaults to
 *  false (off): URLs open in the system browser unless the user opts in. Only an
 *  explicit `'true'` enables the in-app webview. */
export function currentInAppBrowser(): boolean {
  if (preferences.value.status !== 'loaded') return false;
  return preferences.value.data['experimental_in_app_browser'] === 'true';
}

export function setInAppBrowser(enabled: boolean): Promise<void> {
  return savePreference('experimental_in_app_browser', enabled ? 'true' : 'false');
}

/** Whether the in-app browser is the live URL target in this client: the native
 *  webview exists only in the desktop app, AND the experimental toggle has to be
 *  on. The single definition of that pair, so the surfaces that must agree about
 *  it cannot drift: the menu drawer's Browser row (its only entry point) and
 *  `restoreState`'s refusal to resurrect a url-preview overlay on reload.
 *
 *  `openUrl` deliberately does NOT use this: it branches three ways, because
 *  web/PWA opens a tab while the desktop app with the toggle off goes to the OS
 *  opener. */
export function inAppBrowserAvailable(): boolean {
  return isTauri() && currentInAppBrowser();
}

// --- External link target (installed iOS PWA only) ---

/** Where an external http(s) link goes when tapped in an INSTALLED iOS PWA.
 *  Consulted only there: every other client (desktop web, Android, a normal
 *  Safari tab, and all three Tauri branches) opens a new tab / the OS opener
 *  regardless. See `utils/openExternalUrl.ts` for what each mode does and why
 *  the platform forces the choice on us at all. */
export type ExternalLinkTarget = 'safari' | 'ask' | 'in-app';

const EXTERNAL_LINK_TARGETS: readonly ExternalLinkTarget[] = ['safari', 'ask', 'in-app'];

export const DEFAULT_EXTERNAL_LINK_TARGET: ExternalLinkTarget = 'safari';

/** Defaults to `'safari'` (the shipped behaviour) both when unset and while
 *  preferences are still loading, so a link tapped during startup can't fall
 *  into a different mode than the same link tapped a second later. An
 *  unrecognized stored value degrades to the default rather than disabling the
 *  hand-off. */
export function currentExternalLinkTarget(): ExternalLinkTarget {
  if (preferences.value.status !== 'loaded') return DEFAULT_EXTERNAL_LINK_TARGET;
  const stored = preferences.value.data['external_link_target'];
  return EXTERNAL_LINK_TARGETS.includes(stored as ExternalLinkTarget)
    ? stored as ExternalLinkTarget
    : DEFAULT_EXTERNAL_LINK_TARGET;
}

export function setExternalLinkTarget(target: ExternalLinkTarget): Promise<void> {
  return savePreference('external_link_target', target);
}

/** Whether this client is one where the target actually decides anything, and
 *  so the Settings row is worth showing. Only an installed iOS PWA is: every
 *  other client opens a new tab (or the desktop OS opener) whatever the stored
 *  value says. Rendering the row elsewhere would be a control that does nothing.
 *
 *  Deliberately NOT read by `openExternalUrl`, which re-checks `isIOSPwa()` on
 *  its own, so the routing cannot come to depend on a Settings-facing helper. */
export function externalLinkTargetConfigurable(): boolean {
  return isIOSPwa();
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

/** The account default coding agent — the SEED for a fresh compose's backend
 *  chip (via `selectedCodingAgent`, set at `loadPreferences`). Workspace-scoped
 *  (not device-scoped). Default Claude Code — same as the engine's NULL fallback.
 *  Compose picks are per-draft (see `composeSelections`) and deliberately do NOT
 *  write this back (draft-only), so there is no `setCodingAgentDefault`. */
export function currentCodingAgentDefault(): CodingAgent {
  return currentPreference('coding_agent_default', ['claude-code', 'codex'], 'claude-code');
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
